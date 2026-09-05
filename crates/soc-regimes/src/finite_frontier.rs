//! Finite named-candidate deliberation frontier core for finite-decision alpha.
//!
//! # Architecture & Alpha Capabilities
//!
//! 1. **Concrete Named Candidates ([`NamedCandidate`]):**
//!    Candidates carry semantic identification (`name: String`, `regime_id`) alongside
//!    scheduling priorities and canonical interned handles (`src_handle`, `witness_handle`,
//!    `successor_handle`). Order is phase 0, lower numeric priority wins, ties broken by
//!    canonical candidate digest over `(regime_id, witness, successor)`.
//!
//! 2. **Admission Policy & Decisions ([`AdmissionPolicy`], [`AdmissionDecision`]):**
//!    Admission gating via [`AdmitAllPolicy`], [`GuardPolicy`], or custom closures.
//!    Statuses support `selected`, `admitted-not-selected`, and `rejected guard-false`.
//!    Quiescence (all candidates rejected or empty pool) is a valid, non-faulting outcome.
//!
//! 3. **Complete Deliberation Frontier & Caller-Visible Error Path ([`EvaluatedFrontier`], [`EvaluationFault`]):**
//!    Evaluates the complete candidate pool against execution configuration and admission policy.
//!    Under $B^{uk}$ discipline, key conflicts and integrity faults surface through
//!    [`EvaluatedFrontier::selection_outcome`] and [`EvaluatedFrontier::fault`], returning
//!    `Err(EvaluationFault)` so decision publication can be withheld.
//!
//! 4. **Fresh Re-Derivation ([`explain_why`], [`explain_why_not`]):**
//!    Dynamic evaluation of why candidates were admitted/selected or rejected/overshadowed,
//!    evaluated fresh from inputs on every invocation.
//!
//! 5. **SOC Integration ([`FiniteCandidateRegime`], [`PolicyToAdmAdapter`]):**
//!    Provides genuine bridging to `soc-core` calendar, witness providers, and settlement loop.

use core::fmt;
use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{ConfigId, Decomposition, GeneratorId, RegimeId, Witness};

use soc_core::adm::Adm;
use soc_core::calendar::{Frontier, Key, KeyConflict};
use soc_core::commit::{CommitError, SettlementWitnessProvider};
use soc_core::delta::{CandidateDelta, Delta, Footprint};
use soc_core::engine::IncrementalWitnessIndex;
use soc_core::exec::ExecConfig;
use soc_core::intern::{Handle, Interner};
use soc_core::witness_provider::{Candidate, WitnessProvider};

/// Standard tag for canonical key tie-break computation (ADR-0002 §8.1).
pub const CANONICAL_TIEBREAK_TAG: &str = "brix.regimes.NamedCandidate.tiebreak@1";

/// Canonical profile marker for finite-decision alpha (ADR-0030 §2 ⟨D-PROFILE⟩).
pub const FINITE_DECISION_PROFILE_MARKER: &str = "brix.l3.finite-decision@1";

/// Standard regime identifier for finite candidate execution profiles.
pub const FINITE_FRONTIER_REGIME_NAME: &str = "brix.l3.finite-decision@1";

/// Standard generator name for finite candidate decomposition steps.
pub const FINITE_FRONTIER_GENERATOR_NAME: &str = "finite-frontier.step@1";

/// Canonical lowered identity of one finite candidate.
///
/// Human-facing names and scheduling metadata are deliberately absent: the
/// semantic candidate is the versioned regime, witness, and successor triple.
/// The digests are resolved from dense handles once at the calendar boundary.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct CanonicalCandidateV1 {
    /// Versioned realization regime interpreting the witness.
    pub regime_id: RegimeId,
    /// Canonical witness identity resolved from the runtime interner.
    pub witness: Digest,
    /// Canonical successor identity resolved from the runtime interner.
    pub successor: Digest,
}

impl Canonical for CanonicalCandidateV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_tag("brix.regimes.CanonicalCandidate@1");
        self.regime_id.canon_write(w);
        w.write_bytes(self.witness.as_bytes());
        w.write_bytes(self.successor.as_bytes());
    }
}

/// A finite, named candidate for realization deliberation.
///
/// Carries high-level semantic identification alongside lowered, interned handles
/// ready for the `soc-core` calendar and commit loop.
/// Strictly enforces phase 0 in alpha scheduling (ADR-0030 § ⟨D-PHASEZERO⟩).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NamedCandidate {
    /// The user-defined semantic name or label for this candidate.
    pub name: String,
    /// The realization regime under which this candidate's witness is interpreted.
    pub regime_id: RegimeId,
    /// Canonical identity of the source configuration.
    pub src: ConfigId,
    /// Canonical identity of the target/successor configuration.
    pub dst: ConfigId,
    /// Interned handle of the source configuration.
    pub src_handle: Handle,
    /// Interned handle of the witness `w: src → dst` under `regime_id`.
    pub witness_handle: Handle,
    /// Interned handle of the target/successor configuration.
    pub successor_handle: Handle,
    /// Phase index (enforced 0 in alpha scheduling).
    pub phase: u64,
    /// Priority level (smaller value = more urgent / higher priority).
    pub priority: u64,
}

impl NamedCandidate {
    /// Construct and lower a new `NamedCandidate` enforcing phase zero, interning all configuration and witness
    /// identities into `interner`.
    pub fn new(
        name: impl Into<String>,
        regime_id: RegimeId,
        src: ConfigId,
        dst: ConfigId,
        priority: u64,
        interner: &mut Interner,
    ) -> Self {
        let src_handle = interner.intern(src.digest());
        let successor_handle = interner.intern(dst.digest());
        let witness = Witness::new(src, dst, regime_id);
        let witness_handle = interner.intern(witness.id().digest());

        NamedCandidate {
            name: name.into(),
            regime_id,
            src,
            dst,
            src_handle,
            witness_handle,
            successor_handle,
            phase: 0,
            priority,
        }
    }

    /// Construct a `NamedCandidate` with explicitly provided handles, enforcing phase 0.
    #[allow(clippy::too_many_arguments)]
    pub fn with_handles(
        name: impl Into<String>,
        regime_id: RegimeId,
        src: ConfigId,
        dst: ConfigId,
        src_handle: Handle,
        witness_handle: Handle,
        successor_handle: Handle,
        priority: u64,
    ) -> Self {
        NamedCandidate {
            name: name.into(),
            regime_id,
            src,
            dst,
            src_handle,
            witness_handle,
            successor_handle,
            phase: 0,
            priority,
        }
    }

    /// Construct a candidate with an explicit phase, for negative testing of alpha phase-0 enforcement.
    pub fn with_phase_for_test(
        name: impl Into<String>,
        regime_id: RegimeId,
        src: ConfigId,
        dst: ConfigId,
        phase: u64,
        priority: u64,
        interner: &mut Interner,
    ) -> Self {
        let src_handle = interner.intern(src.digest());
        let successor_handle = interner.intern(dst.digest());
        let witness = Witness::new(src, dst, regime_id);
        let witness_handle = interner.intern(witness.id().digest());

        NamedCandidate {
            name: name.into(),
            regime_id,
            src,
            dst,
            src_handle,
            witness_handle,
            successor_handle,
            phase,
            priority,
        }
    }

    /// Project this named candidate to a lean `soc_core::witness_provider::Candidate`.
    pub fn to_candidate(&self) -> Candidate {
        Candidate {
            witness: self.witness_handle,
            successor: self.successor_handle,
        }
    }

    /// Resolve this candidate to the canonical `(regime, witness, successor)`
    /// identity used by the execution profile.
    pub fn canonical_identity(&self, interner: &Interner) -> CanonicalCandidateV1 {
        CanonicalCandidateV1 {
            regime_id: self.regime_id,
            witness: interner.resolve(self.witness_handle),
            successor: interner.resolve(self.successor_handle),
        }
    }

    /// Compute the boundary-resolved canonical tie-break digest through [`Interner`].
    ///
    /// Resolves `witness_handle` and `successor_handle` to their canonical digests stored
    /// in the interner, combining them with the regime identity.
    /// Human names and numerical priorities are deliberately excluded from this tiebreak digest.
    pub fn canonical_tiebreak(&self, interner: &Interner) -> Digest {
        let mut w = CanonWriter::new();
        w.write_tag(CANONICAL_TIEBREAK_TAG);
        self.canonical_identity(interner).canon_write(&mut w);
        w.digest(Domain::Value)
    }

    /// Derive the full SOC calendar [`Key`] `(phase, priority, tiebreak)` deterministically.
    ///
    /// Lower numerical priority values win. Equal phase and priority tie-break
    /// deterministically on canonical candidate digest.
    pub fn canonical_key(&self, interner: &Interner) -> Key {
        Key::new(self.phase, self.priority, self.canonical_tiebreak(interner))
    }
}

impl From<&NamedCandidate> for Candidate {
    fn from(nc: &NamedCandidate) -> Self {
        nc.to_candidate()
    }
}

impl From<NamedCandidate> for Candidate {
    fn from(nc: NamedCandidate) -> Self {
        nc.to_candidate()
    }
}

/// Structured outcome status of an evaluated candidate.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum CandidateStatus {
    /// The candidate was admitted and selected as the unique least Key.
    Selected,
    /// The candidate was admitted, but was not selected because another candidate had a smaller Key.
    AdmittedNotSelected,
    /// The candidate was rejected because its policy guard evaluated to `false`.
    RejectedGuardFalse,
    /// The candidate was rejected with a structured reason code (preserves custom rejection reason).
    Rejected(ReasonCode),
}

impl CandidateStatus {
    /// Borrow the rejection reason code, if this status represents a rejection.
    pub fn rejection_reason(&self) -> Option<&ReasonCode> {
        match self {
            CandidateStatus::RejectedGuardFalse => Some(&ReasonCode::GuardFalse),
            CandidateStatus::Rejected(r) => Some(r),
            _ => None,
        }
    }
}

impl fmt::Display for CandidateStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CandidateStatus::Selected => write!(f, "selected"),
            CandidateStatus::AdmittedNotSelected => write!(f, "admitted-not-selected"),
            CandidateStatus::RejectedGuardFalse => write!(f, "rejected guard-false"),
            CandidateStatus::Rejected(reason) => write!(f, "rejected ({reason})"),
        }
    }
}

/// Deterministic reason codes for candidate rejection.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ReasonCode {
    /// Candidate was rejected because the guard predicate evaluated to `false`.
    GuardFalse,
    /// Custom, domain-specific structured reason code.
    Custom {
        /// Stable reason code tag.
        code: &'static str,
        /// Detailed description or context.
        detail: String,
    },
}

impl ReasonCode {
    /// Return a stable static identifier for this reason code category.
    pub const fn category(&self) -> &'static str {
        match self {
            ReasonCode::GuardFalse => "guard_false@1",
            ReasonCode::Custom { code, .. } => code,
        }
    }
}

impl fmt::Display for ReasonCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReasonCode::GuardFalse => write!(f, "rejected guard-false"),
            ReasonCode::Custom { code, detail } => write!(f, "{code}: {detail}"),
        }
    }
}

impl Canonical for ReasonCode {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            ReasonCode::GuardFalse => {
                w.write_enum(0, |_| {});
            }
            ReasonCode::Custom { code, detail } => {
                w.write_enum(1, |w| {
                    w.write_str(code);
                    w.write_str(detail);
                });
            }
        }
    }
}

/// The decision of an admission evaluation.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum AdmissionDecision {
    /// Candidate is admitted to the deliberation frontier.
    Admitted,
    /// Candidate is rejected with a structured reason code.
    Rejected(ReasonCode),
    /// Policy evaluation encountered a fault or evaluation error.
    Error(String),
}

impl AdmissionDecision {
    /// Returns `true` if the decision is [`AdmissionDecision::Admitted`].
    pub fn is_admitted(&self) -> bool {
        matches!(self, AdmissionDecision::Admitted)
    }

    /// Returns `true` if the decision is [`AdmissionDecision::Rejected`].
    pub fn is_rejected(&self) -> bool {
        matches!(self, AdmissionDecision::Rejected(_))
    }

    /// Returns `true` if the decision is [`AdmissionDecision::Error`].
    pub fn is_error(&self) -> bool {
        matches!(self, AdmissionDecision::Error(_))
    }

    /// Construct a rejected decision with [`ReasonCode::GuardFalse`].
    pub fn rejected_guard_false() -> Self {
        AdmissionDecision::Rejected(ReasonCode::GuardFalse)
    }

    /// Construct a rejected decision with a custom structured reason.
    pub fn rejected_custom(code: &'static str, detail: impl Into<String>) -> Self {
        AdmissionDecision::Rejected(ReasonCode::Custom {
            code,
            detail: detail.into(),
        })
    }

    /// Construct an error decision representing an admission evaluation failure.
    pub fn error(detail: impl Into<String>) -> Self {
        AdmissionDecision::Error(detail.into())
    }

    /// Borrow the rejection reason, if any.
    pub fn rejection_reason(&self) -> Option<&ReasonCode> {
        match self {
            AdmissionDecision::Rejected(r) => Some(r),
            _ => None,
        }
    }
}

/// A first-class admission policy evaluated over execution configurations and named candidates.
pub trait AdmissionPolicy {
    /// Evaluate whether candidate `c` is admitted under execution configuration `e`.
    fn evaluate(&self, e: &ExecConfig, c: &NamedCandidate) -> AdmissionDecision;
}

/// An admission policy that admits every candidate unconditionally.
#[derive(Clone, Copy, Debug, Default)]
pub struct AdmitAllPolicy;

impl AdmissionPolicy for AdmitAllPolicy {
    fn evaluate(&self, _e: &ExecConfig, _c: &NamedCandidate) -> AdmissionDecision {
        AdmissionDecision::Admitted
    }
}

/// An admission policy governed by a boolean guard predicate.
///
/// Returns [`AdmissionDecision::Admitted`] when the guard returns `true`,
/// and [`AdmissionDecision::Rejected(ReasonCode::GuardFalse)`] when `false`.
#[derive(Clone, Copy, Debug)]
pub struct GuardPolicy<F>(pub F);

impl<F: Fn(&ExecConfig, &NamedCandidate) -> bool> AdmissionPolicy for GuardPolicy<F> {
    fn evaluate(&self, e: &ExecConfig, c: &NamedCandidate) -> AdmissionDecision {
        if (self.0)(e, c) {
            AdmissionDecision::Admitted
        } else {
            AdmissionDecision::rejected_guard_false()
        }
    }
}

/// An admission policy that rejects every candidate with [`ReasonCode::GuardFalse`].
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyAllPolicy;

impl AdmissionPolicy for DenyAllPolicy {
    fn evaluate(&self, _e: &ExecConfig, _c: &NamedCandidate) -> AdmissionDecision {
        AdmissionDecision::rejected_guard_false()
    }
}

/// An admission policy backed by a closure returning an [`AdmissionDecision`].
pub struct FnPolicy<F>(pub F);

impl<F: Fn(&ExecConfig, &NamedCandidate) -> AdmissionDecision> AdmissionPolicy for FnPolicy<F> {
    fn evaluate(&self, e: &ExecConfig, c: &NamedCandidate) -> AdmissionDecision {
        (self.0)(e, c)
    }
}

/// Caller-visible evaluation fault detected during deliberation frontier evaluation.
///
/// Under $B^{uk}$ discipline (ADR-0002 §5.3, §8; ADR-0030 §2 ⟨D-FAILCLOSED⟩),
/// when an evaluation fault occurs, execution halts and decision publication must be withheld.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EvaluationFault {
    /// Key collision under B^uk discipline between two named candidates.
    KeyConflict(KeyConflict<NamedCandidate>),
    /// Key collision in the projected lean SOC candidate frontier.
    CandidateKeyConflict(KeyConflict<Candidate>),
    /// Failure during admission policy evaluation.
    AdmissionError {
        /// Candidate during whose admission the error occurred.
        candidate: NamedCandidate,
        /// Description of the admission error.
        detail: String,
    },
    /// General evaluation fault during deliberation (e.g. arithmetic fault, runtime error).
    EvaluationError {
        /// Detail of the evaluation error.
        detail: String,
    },
    /// Non-zero phase encountered on candidate in alpha profile (ADR-0030 § ⟨D-PHASEZERO⟩).
    InvalidPhase {
        /// Candidate with non-zero phase.
        candidate: NamedCandidate,
        /// Phase value declared.
        phase: u64,
    },
}

impl EvaluationFault {
    /// Construct an admission error fault.
    pub fn admission_error(candidate: NamedCandidate, detail: impl Into<String>) -> Self {
        EvaluationFault::AdmissionError {
            candidate,
            detail: detail.into(),
        }
    }

    /// Construct a general deliberation evaluation error fault.
    pub fn evaluation_error(detail: impl Into<String>) -> Self {
        EvaluationFault::EvaluationError {
            detail: detail.into(),
        }
    }

    /// Construct an invalid phase fault.
    pub fn invalid_phase(candidate: NamedCandidate, phase: u64) -> Self {
        EvaluationFault::InvalidPhase { candidate, phase }
    }
}

impl fmt::Display for EvaluationFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvaluationFault::KeyConflict(c) => write!(
                f,
                "Key conflict under B^uk discipline at key (phase {}, priority {}): existing '{}' collided with attempted '{}'",
                c.key.phase, c.key.priority, c.existing.name, c.attempted.name
            ),
            EvaluationFault::CandidateKeyConflict(c) => write!(
                f,
                "Projected candidate key conflict at key (phase {}, priority {}): existing witness {} collided with attempted witness {}",
                c.key.phase, c.key.priority, c.existing.witness.raw(), c.attempted.witness.raw()
            ),
            EvaluationFault::AdmissionError { candidate, detail } => write!(
                f,
                "Admission evaluation error for candidate '{}': {detail}",
                candidate.name
            ),
            EvaluationFault::EvaluationError { detail } => write!(
                f,
                "Deliberation evaluation error: {detail}"
            ),
            EvaluationFault::InvalidPhase { candidate, phase } => write!(
                f,
                "Candidate '{}' declared invalid non-zero phase {phase} (alpha profile enforces phase 0)",
                candidate.name
            ),
        }
    }
}

impl std::error::Error for EvaluationFault {}

/// Explicit outcome of finite deliberation usable by the lowerer (ADR-0030 § ⟨D-QUIESCENCE⟩).
///
/// Deliberation yields either a cleanly selected minimal-key candidate or an explicit
/// quiescent state when all candidates are rejected or the candidate pool is empty.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DeliberationOutcome {
    /// Exactly one winning candidate was selected from the frontier.
    Selected {
        /// Canonical key of winning candidate.
        key: Key,
        /// Winning named candidate.
        candidate: NamedCandidate,
    },
    /// All candidates were rejected (or pool empty); explicit non-faulting quiescence.
    Quiescent,
}

impl DeliberationOutcome {
    /// Returns `true` if this outcome is [`DeliberationOutcome::Selected`].
    pub fn is_selected(&self) -> bool {
        matches!(self, DeliberationOutcome::Selected { .. })
    }

    /// Returns `true` if this outcome is [`DeliberationOutcome::Quiescent`].
    pub fn is_quiescent(&self) -> bool {
        matches!(self, DeliberationOutcome::Quiescent)
    }

    /// Borrow the selected candidate, if any.
    pub fn selected_candidate(&self) -> Option<&NamedCandidate> {
        match self {
            DeliberationOutcome::Selected { candidate, .. } => Some(candidate),
            DeliberationOutcome::Quiescent => None,
        }
    }

    /// Copy the selected key, if any.
    pub fn selected_key(&self) -> Option<Key> {
        match self {
            DeliberationOutcome::Selected { key, .. } => Some(*key),
            DeliberationOutcome::Quiescent => None,
        }
    }
}

/// Complete deliberation evaluation of a candidate pool under an execution configuration.
#[derive(Clone, Debug)]
pub struct EvaluatedFrontier {
    /// Execution configuration the deliberation was evaluated under.
    pub config: ExecConfig,
    /// All admitted named candidates mapped by their canonical [`Key`].
    pub admitted: BTreeMap<Key, NamedCandidate>,
    /// All rejected named candidates mapped to their structured rejection reason.
    pub rejected: BTreeMap<NamedCandidate, ReasonCode>,
    /// Unique-key deliberation frontier built for admitted named candidates.
    pub frontier: Frontier<NamedCandidate>,
    /// Unique-key deliberation frontier projected to lean `Candidate`s.
    pub candidate_frontier: Frontier<Candidate>,
    /// Deterministically selected least candidate (`select_K`), or `None` if frontier is empty
    /// or if evaluation faulted.
    pub selected: Option<(Key, NamedCandidate)>,
    /// Any key conflicts detected during frontier insertion under $B^{uk}$ discipline.
    pub key_conflicts: Vec<KeyConflict<NamedCandidate>>,
    /// Key conflicts in the lean SOC candidate projection.
    pub candidate_key_conflicts: Vec<KeyConflict<Candidate>>,
    /// Any non-key-conflict evaluation faults (admission errors, phase errors, etc.).
    pub evaluation_faults: Vec<EvaluationFault>,
}

impl EvaluatedFrontier {
    /// Evaluate the COMPLETE candidate set against `e` and `policy`, building the deliberation frontier.
    pub fn evaluate<P: AdmissionPolicy>(
        candidates: impl IntoIterator<Item = NamedCandidate>,
        policy: &P,
        e: &ExecConfig,
        interner: &Interner,
    ) -> Self {
        let mut admitted = BTreeMap::new();
        let mut rejected = BTreeMap::new();
        let mut frontier = Frontier::new();
        let mut candidate_frontier = Frontier::new();
        let mut key_conflicts = Vec::new();
        let mut candidate_key_conflicts = Vec::new();
        let mut evaluation_faults = Vec::new();

        // Normalize enumeration order before policy evaluation and keying so
        // conflict reports and representative metadata are deterministic.
        let ordered: BTreeSet<_> = candidates.into_iter().collect();
        for c in ordered {
            // Enforce phase zero in alpha profile (ADR-0030 § ⟨D-PHASEZERO⟩).
            if c.phase != 0 {
                evaluation_faults.push(EvaluationFault::InvalidPhase {
                    candidate: c.clone(),
                    phase: c.phase,
                });
                continue;
            }

            match policy.evaluate(e, &c) {
                AdmissionDecision::Error(detail) => {
                    evaluation_faults.push(EvaluationFault::AdmissionError {
                        candidate: c,
                        detail,
                    });
                }
                AdmissionDecision::Rejected(reason) => {
                    rejected.insert(c, reason);
                }
                AdmissionDecision::Admitted => {
                    let key = c.canonical_key(interner);
                    admitted.entry(key).or_insert_with(|| c.clone());
                    if let Err(conflict) = frontier.insert(key, c.clone()) {
                        key_conflicts.push(conflict);
                    }
                    if let Err(conflict) = candidate_frontier.insert(key, c.to_candidate()) {
                        candidate_key_conflicts.push(conflict);
                    }
                }
            }
        }

        // Fail closed: if any key collision, admission error, or phase fault occurred,
        // withhold decision publication.
        let selected = if key_conflicts.is_empty()
            && candidate_key_conflicts.is_empty()
            && evaluation_faults.is_empty()
        {
            frontier.peek_least().map(|(k, v)| (*k, v.clone()))
        } else {
            None
        };

        EvaluatedFrontier {
            config: *e,
            admitted,
            rejected,
            frontier,
            candidate_frontier,
            selected,
            key_conflicts,
            candidate_key_conflicts,
            evaluation_faults,
        }
    }

    /// Evaluate the complete candidate set, returning `Err(EvaluationFault)` if any evaluation fault occurred.
    #[allow(clippy::result_large_err)]
    pub fn try_evaluate<P: AdmissionPolicy>(
        candidates: impl IntoIterator<Item = NamedCandidate>,
        policy: &P,
        e: &ExecConfig,
        interner: &Interner,
    ) -> Result<Self, EvaluationFault> {
        let evaluated = Self::evaluate(candidates, policy, e, interner);
        if let Some(fault) = evaluated.fault() {
            Err(fault)
        } else {
            Ok(evaluated)
        }
    }

    /// Return the first evaluation fault, if any faults or key conflicts occurred.
    pub fn fault(&self) -> Option<EvaluationFault> {
        if let Some(f) = self.evaluation_faults.first() {
            Some(f.clone())
        } else if let Some(c) = self.key_conflicts.first() {
            Some(EvaluationFault::KeyConflict(c.clone()))
        } else {
            self.candidate_key_conflicts
                .first()
                .map(|c| EvaluationFault::CandidateKeyConflict(c.clone()))
        }
    }

    /// Explicit deliberation outcome usable by the lowerer (ADR-0030 § ⟨D-QUIESCENCE⟩).
    /// - `Ok(DeliberationOutcome::Selected { key, candidate })` if a candidate was cleanly selected.
    /// - `Ok(DeliberationOutcome::Quiescent)` if no candidate was admitted (valid quiescence).
    /// - `Err(EvaluationFault)` if deliberation faulted (key conflict, admission error, etc.).
    #[allow(clippy::result_large_err)]
    pub fn deliberation_outcome(&self) -> Result<DeliberationOutcome, EvaluationFault> {
        if let Some(fault) = self.fault() {
            Err(fault)
        } else {
            match &self.selected {
                Some((key, candidate)) => Ok(DeliberationOutcome::Selected {
                    key: *key,
                    candidate: candidate.clone(),
                }),
                None => Ok(DeliberationOutcome::Quiescent),
            }
        }
    }

    /// Caller-visible decision outcome path:
    /// - Returns `Ok(Some((key, candidate)))` if a candidate was cleanly selected.
    /// - Returns `Ok(None)` if no candidates were admitted (valid quiescence).
    /// - Returns `Err(EvaluationFault)` if an evaluation fault occurred so decision publication can be withheld.
    #[allow(clippy::result_large_err)]
    pub fn selection_outcome(&self) -> Result<Option<(Key, NamedCandidate)>, EvaluationFault> {
        if let Some(fault) = self.fault() {
            Err(fault)
        } else {
            Ok(self.selected.clone())
        }
    }

    /// Determine the candidate outcome status for candidate `c`.
    /// Preserves the actual structured reason code for custom rejections.
    pub fn status_of(&self, c: &NamedCandidate) -> Option<CandidateStatus> {
        if let Some((_, sel)) = &self.selected {
            if sel == c {
                return Some(CandidateStatus::Selected);
            }
        }
        if self.admitted.values().any(|cand| cand == c) {
            return Some(CandidateStatus::AdmittedNotSelected);
        }
        if let Some(reason) = self.rejected.get(c) {
            return match reason {
                ReasonCode::GuardFalse => Some(CandidateStatus::RejectedGuardFalse),
                custom => Some(CandidateStatus::Rejected(custom.clone())),
            };
        }
        None
    }

    /// Whether no candidate was selected (valid quiescence or empty frontier).
    pub fn is_quiescent(&self) -> bool {
        self.selected.is_none() && self.fault().is_none()
    }

    /// Number of admitted candidates.
    pub fn admitted_count(&self) -> usize {
        self.admitted.len()
    }

    /// Number of rejected candidates.
    pub fn rejected_count(&self) -> usize {
        self.rejected.len()
    }

    /// Total number of candidates evaluated.
    pub fn total_count(&self) -> usize {
        self.admitted.len() + self.rejected.len()
    }
}

/// Structured explanation of why a candidate was admitted and/or selected.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum WhyExplanation {
    /// The candidate was admitted and selected as the unique least Key in the deliberation frontier.
    Selected {
        /// Canonical key of the selected candidate.
        key: Key,
        /// The candidate itself.
        candidate: NamedCandidate,
    },
    /// The candidate was admitted, but was not selected because a candidate with a smaller Key won.
    AdmittedNotSelected {
        /// Canonical key of this candidate.
        key: Key,
        /// The candidate itself.
        candidate: NamedCandidate,
        /// Canonical key of the winning candidate.
        selected_key: Key,
        /// The winning candidate.
        selected_candidate: NamedCandidate,
    },
    /// The candidate was not admitted under the policy at this configuration.
    NotAdmitted {
        /// The candidate evaluated.
        candidate: NamedCandidate,
        /// Rejection reason code.
        reason: ReasonCode,
    },
    /// The requested candidate is absent from the freshly enumerated pool.
    CandidateNotFound,
    /// Deliberation faulted (e.g. key conflict under B^uk discipline or evaluation fault).
    EvaluationFaulted {
        /// The target candidate.
        candidate: NamedCandidate,
        /// The fault that occurred.
        fault: EvaluationFault,
    },
    /// No candidates were admitted to the deliberation frontier (valid quiescence).
    NoCandidateAdmitted,
}

/// Structured explanation of why a candidate was NOT admitted or NOT selected.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum WhyNotExplanation {
    /// The candidate was rejected by the admission policy with this structured reason.
    RejectedByPolicy {
        /// The candidate evaluated.
        candidate: NamedCandidate,
        /// Structured rejection reason.
        reason: ReasonCode,
    },
    /// The candidate was admitted, but lost selection to a higher-priority / smaller-key candidate.
    Overshadowed {
        /// The candidate evaluated.
        candidate: NamedCandidate,
        /// Canonical key of this candidate.
        key: Key,
        /// Canonical key of the winning candidate.
        selected_key: Key,
        /// The winning candidate.
        selected_candidate: NamedCandidate,
    },
    /// The candidate was actually selected (it was NOT rejected or overshadowed).
    ActuallySelected {
        /// The candidate evaluated.
        candidate: NamedCandidate,
        /// Canonical key of this candidate.
        key: Key,
    },
    /// The candidate was not found in the candidate pool.
    CandidateNotFound,
    /// Deliberation faulted (e.g. key conflict under B^uk discipline or evaluation fault).
    EvaluationFaulted {
        /// The target candidate.
        candidate: NamedCandidate,
        /// The fault that occurred.
        fault: EvaluationFault,
    },
    /// No candidate was selected because the admitted set was empty (valid quiescence).
    NoCandidateSelected,
}

/// Re-derive why a candidate was admitted/selected fresh from current inputs.
///
/// Never relies on cached or stored explanation strings.
/// When an evaluation fault or key conflict occurs, returns `EvaluationFaulted` and
/// never reports `Selected` or `CandidateNotFound` for an existing candidate.
pub fn explain_why<P: AdmissionPolicy>(
    candidates: &[NamedCandidate],
    policy: &P,
    e: &ExecConfig,
    target: &NamedCandidate,
    interner: &Interner,
) -> WhyExplanation {
    if !candidates.contains(target) {
        return WhyExplanation::CandidateNotFound;
    }
    let frontier = EvaluatedFrontier::evaluate(candidates.iter().cloned(), policy, e, interner);
    if let Some(fault) = frontier.fault() {
        return WhyExplanation::EvaluationFaulted {
            candidate: target.clone(),
            fault,
        };
    }
    match policy.evaluate(e, target) {
        AdmissionDecision::Error(detail) => WhyExplanation::EvaluationFaulted {
            candidate: target.clone(),
            fault: EvaluationFault::AdmissionError {
                candidate: target.clone(),
                detail,
            },
        },
        AdmissionDecision::Rejected(reason) => WhyExplanation::NotAdmitted {
            candidate: target.clone(),
            reason,
        },
        AdmissionDecision::Admitted => {
            let target_key = target.canonical_key(interner);
            match frontier.selected {
                Some((sel_key, sel_cand)) if sel_key == target_key && sel_cand == *target => {
                    WhyExplanation::Selected {
                        key: target_key,
                        candidate: target.clone(),
                    }
                }
                Some((sel_key, sel_cand)) => WhyExplanation::AdmittedNotSelected {
                    key: target_key,
                    candidate: target.clone(),
                    selected_key: sel_key,
                    selected_candidate: sel_cand,
                },
                None => WhyExplanation::NoCandidateAdmitted,
            }
        }
    }
}

/// Re-derive why a candidate was NOT admitted or NOT selected fresh from current inputs.
///
/// Never relies on cached or stored explanation strings.
/// When an evaluation fault or key conflict occurs, returns `EvaluationFaulted` and
/// never reports `ActuallySelected` or `CandidateNotFound` for an existing candidate.
pub fn explain_why_not<P: AdmissionPolicy>(
    candidates: &[NamedCandidate],
    policy: &P,
    e: &ExecConfig,
    target: &NamedCandidate,
    interner: &Interner,
) -> WhyNotExplanation {
    if !candidates.contains(target) {
        return WhyNotExplanation::CandidateNotFound;
    }
    let frontier = EvaluatedFrontier::evaluate(candidates.iter().cloned(), policy, e, interner);
    if let Some(fault) = frontier.fault() {
        return WhyNotExplanation::EvaluationFaulted {
            candidate: target.clone(),
            fault,
        };
    }
    match policy.evaluate(e, target) {
        AdmissionDecision::Error(detail) => WhyNotExplanation::EvaluationFaulted {
            candidate: target.clone(),
            fault: EvaluationFault::AdmissionError {
                candidate: target.clone(),
                detail,
            },
        },
        AdmissionDecision::Rejected(reason) => WhyNotExplanation::RejectedByPolicy {
            candidate: target.clone(),
            reason,
        },
        AdmissionDecision::Admitted => {
            let target_key = target.canonical_key(interner);
            match frontier.selected {
                Some((sel_key, sel_cand)) if sel_key == target_key && sel_cand == *target => {
                    WhyNotExplanation::ActuallySelected {
                        candidate: target.clone(),
                        key: target_key,
                    }
                }
                Some((sel_key, sel_cand)) => WhyNotExplanation::Overshadowed {
                    candidate: target.clone(),
                    key: target_key,
                    selected_key: sel_key,
                    selected_candidate: sel_cand,
                },
                None => WhyNotExplanation::NoCandidateSelected,
            }
        }
    }
}

/// Adapter presenting an [`AdmissionPolicy`] as a boolean `soc_core::adm::Adm`.
pub struct PolicyToAdmAdapter<'a, P> {
    /// Borrowed admission policy.
    pub policy: &'a P,
    /// Map of witness handle to named candidate.
    pub candidates_by_witness: BTreeMap<Handle, NamedCandidate>,
}

impl<'a, P> PolicyToAdmAdapter<'a, P> {
    /// Construct a new adapter over a policy and an iterable of known named candidates.
    pub fn new(policy: &'a P, candidates: impl IntoIterator<Item = NamedCandidate>) -> Self {
        let mut map = BTreeMap::new();
        for nc in candidates {
            map.insert(nc.witness_handle, nc);
        }
        PolicyToAdmAdapter {
            policy,
            candidates_by_witness: map,
        }
    }
}

impl<'a, P: AdmissionPolicy> Adm for PolicyToAdmAdapter<'a, P> {
    fn admits(&self, e: &ExecConfig, c: &Candidate) -> bool {
        if let Some(nc) = self.candidates_by_witness.get(&c.witness) {
            self.policy.evaluate(e, nc).is_admitted()
        } else {
            false
        }
    }
}

/// A finite realization regime providing candidate enumeration and incremental dataflow execution.
#[derive(Clone, Debug)]
pub struct FiniteCandidateRegime {
    regime_id: RegimeId,
    candidates_by_src: BTreeMap<Handle, Vec<NamedCandidate>>,
    all_candidates: BTreeSet<NamedCandidate>,
    known_configs: BTreeMap<Handle, ConfigId>,
}

impl FiniteCandidateRegime {
    /// Canonical name of the finite frontier realization regime.
    pub const NAME: &'static str = FINITE_FRONTIER_REGIME_NAME;

    /// Canonical generator name emitted for candidate decomposition steps.
    pub const GENERATOR_NAME: &'static str = FINITE_FRONTIER_GENERATOR_NAME;

    /// Construct a new finite candidate regime with the given `RegimeId`.
    pub fn new(regime_id: RegimeId) -> Self {
        FiniteCandidateRegime {
            regime_id,
            candidates_by_src: BTreeMap::new(),
            all_candidates: BTreeSet::new(),
            known_configs: BTreeMap::new(),
        }
    }

    /// Construct a fresh regime using the standard [`FINITE_FRONTIER_REGIME_NAME`].
    pub fn default_regime(_interner: &mut Interner) -> Self {
        Self::new(RegimeId::named(Self::NAME))
    }

    /// This regime's canonical identity.
    pub fn regime_id(&self) -> RegimeId {
        self.regime_id
    }

    /// Register a configuration as known to this regime.
    pub fn register_config(&mut self, interner: &mut Interner, config: ConfigId) -> Handle {
        let handle = interner.intern(config.digest());
        self.known_configs.insert(handle, config);
        handle
    }

    /// Add an already-lowered `NamedCandidate` to this regime's candidate pool.
    pub fn add_candidate(&mut self, candidate: NamedCandidate) {
        assert_eq!(
            candidate.regime_id, self.regime_id,
            "finite candidate must be lowered under this regime's identity"
        );
        assert_eq!(
            candidate.phase, 0,
            "finite candidate must be at phase zero in alpha profile"
        );
        self.known_configs
            .insert(candidate.src_handle, candidate.src);
        self.known_configs
            .insert(candidate.successor_handle, candidate.dst);
        self.candidates_by_src
            .entry(candidate.src_handle)
            .or_default()
            .push(candidate.clone());
        self.all_candidates.insert(candidate);
    }

    /// Construct, lower, and register a new named candidate into this regime enforcing phase zero.
    pub fn register_named_candidate(
        &mut self,
        interner: &mut Interner,
        name: impl Into<String>,
        src: ConfigId,
        dst: ConfigId,
        priority: u64,
    ) -> NamedCandidate {
        let candidate = NamedCandidate::new(name, self.regime_id, src, dst, priority, interner);
        self.add_candidate(candidate.clone());
        candidate
    }

    /// Return all registered named candidates.
    pub fn all_candidates(&self) -> &BTreeSet<NamedCandidate> {
        &self.all_candidates
    }

    /// Return candidates keyed by source handle.
    pub fn candidates_for_source(&self, src: Handle) -> Option<&[NamedCandidate]> {
        self.candidates_by_src.get(&src).map(|v| v.as_slice())
    }
}

impl WitnessProvider for FiniteCandidateRegime {
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        self.candidates_by_src
            .get(&e.world)
            .map(|list| list.iter().map(|nc| nc.to_candidate()).collect())
            .unwrap_or_default()
    }
}

impl SettlementWitnessProvider for FiniteCandidateRegime {
    fn try_decompose(&self, e: &ExecConfig, c: &Candidate) -> Result<Decomposition, CommitError> {
        let named = self
            .candidates_by_src
            .get(&e.world)
            .and_then(|candidates| {
                candidates
                    .iter()
                    .find(|candidate| candidate.to_candidate() == *c)
            })
            .ok_or(CommitError::CandidateMismatch)?;
        if named.regime_id != self.regime_id {
            return Err(CommitError::WitnessMismatch);
        }

        Decomposition::recorded(
            vec![GeneratorId::named(Self::GENERATOR_NAME)],
            vec![named.src, named.dst],
        )
        .map_err(
            |brix_semantic::DecompositionError::ChainLengthMismatch {
                 generators,
                 configs,
             }| CommitError::ChainLengthMismatch {
                generators,
                configs,
            },
        )
    }
}

impl IncrementalWitnessIndex for FiniteCandidateRegime {
    fn footprint(&self) -> Footprint {
        Footprint::configs(self.candidates_by_src.keys().copied())
    }

    fn apply(&mut self, delta: &Delta) -> CandidateDelta {
        let mut cd = CandidateDelta::new();
        for h in &delta.added {
            if let Some(list) = self.candidates_by_src.get(h) {
                for nc in list {
                    cd.added.insert(nc.to_candidate());
                }
            }
        }
        for h in &delta.removed {
            if let Some(list) = self.candidates_by_src.get(h) {
                for nc in list {
                    cd.removed.insert(nc.to_candidate());
                }
            }
        }
        cd
    }
}
