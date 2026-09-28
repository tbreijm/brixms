//! Finite-decision alpha deliberation runtime and settlement integration (ADR-0030).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use brix_canon::{CanonWriter, Digest, Domain};
use brix_semantic::{
    ConfigId, ContextId, Decomposition, GeneratorId, GeneratorRegistry, Outcome, RegimeId, Witness,
};

use soc_core::adm::AdmAll;
use soc_core::audit::{audit_journal, AuditResult, GeneratorSemanticsV1};
use soc_core::calendar::Key;
use soc_core::commit::{try_commit_tick, CommitError, Committed, SettlementWitnessProvider};
use soc_core::exec::ExecConfig;
use soc_core::history::History;
use soc_core::intern::{Handle, Interner};
use soc_core::journal::{CommittedStep, Journal};
use soc_core::saturate::{
    check_quiescence_certificate, quiescence_certificate_id, sat_step, CertificateCheck,
    DeclaredAssumptions, GeneratorPartitionProfile, PresentationIdV1, PresentationV1,
    QuiescenceCertificateId, SaturatedStep, SaturationBudget,
};
use soc_core::witness_provider::{Candidate, WitnessProvider};
use soc_regimes::finite_frontier::{
    CandidateStatus, EvaluatedFrontier, EvaluationFault, FnPolicy, NamedCandidate,
    PolicyToAdmAdapter, WhyExplanation, WhyNotExplanation, FINITE_FRONTIER_REGIME_NAME,
};
use soc_regimes::{explain_why, explain_why_not};

use crate::finite_decision::plan::{
    finite_decision_program_id, FiniteDecisionCommit, FiniteDecisionPlan, FiniteDecisionProgramId,
};
use crate::input::{input_context_id, InputSnapshot, InputValidationError};
use crate::l3_v2::{eval, EvalEnv, EvalFault, L3ExprV2, L3FunctionDef, L3SchemaType, L3ValueV2};

const WORLD_MARKER: &[u8] = b"brix.l3.finite-decision.world";
const POLICY_MARKER: &[u8] = b"brix.l3.finite-decision.adm-all";
const GENERATOR_TAG: &str = "brix.l3.finite-decision.generator@1";

/// A bound external input, published strictly at [`Outcome::Derived`] (ADR-0031).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundInput {
    pub ordinal: u64,
    pub name: String,
    pub ty: L3ValueType,
    pub value: L3ValueV2,
    pub grade: Outcome,
}

/// Error encountered during finite-decision runtime construction (ADR-0031).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FiniteDecisionBuildError {
    InputValidation(InputValidationError),
    MissingProposal { candidate: String },
}

impl fmt::Display for FiniteDecisionBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InputValidation(err) => write!(f, "input validation failed: {err}"),
            Self::MissingProposal { candidate } => {
                write!(
                    f,
                    "candidate '{candidate}' in commit was not found in declared proposals"
                )
            }
        }
    }
}

impl std::error::Error for FiniteDecisionBuildError {}

impl FiniteDecisionBuildError {
    /// The declared item this error is about (see
    /// [`FiniteDecisionLowerError::location_subject`] for the shared design):
    /// the `input` declaration for a wrapped [`InputValidationError`], or the
    /// `commit` declaration for a missing proposal (the candidate name itself
    /// is a `propose`, not the commit, but it is the commit's candidate list
    /// that names it, so that is the more useful line to point at).
    pub fn location_subject(&self) -> Option<(&str, Option<&str>)> {
        match self {
            Self::InputValidation(err) => err.location_subject(),
            Self::MissingProposal { candidate } => Some((candidate, None)),
        }
    }
}

impl From<InputValidationError> for FiniteDecisionBuildError {
    fn from(err: InputValidationError) -> Self {
        Self::InputValidation(err)
    }
}

pub use crate::l3_v2::{type_of_value, L3ValueType};

/// A fact committed by rule derivation, published strictly at [`Outcome::Derived`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivedFact {
    pub rule: String,
    pub value: L3ValueV2,
    pub ordinal: u64,
    pub grade: Outcome,
}

/// The winning decision selected by deliberation, published strictly at [`Outcome::Derived`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedDecision {
    pub candidate: String,
    pub priority: u64,
    pub value: L3ValueV2,
    pub grade: Outcome,
}

/// Structured disposition for a candidate proposal in the commit pool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateDisposition {
    pub name: String,
    pub priority: u64,
    pub status: CandidateStatus,
}

/// Why a finite-decision deliberation resulted in Unknown and published no decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FiniteDecisionUnknownReason {
    ExpressionEvaluationFault { context: String, fault: EvalFault },
    DependencyFault { context: String, detail: String },
    TypeFault { context: String, detail: String },
    DecisionKeyConflict { detail: String },
    AdmissionError { candidate: String, detail: String },
    EvaluationError { detail: String },
    InvalidPhase { candidate: String, phase: u64 },
    QuiescenceVerificationFault { detail: String },
    CommitTickError { detail: String },
    InvariantViolation { detail: String },
}

impl FiniteDecisionUnknownReason {
    /// Whether this reason is an expression evaluation fault.
    pub fn is_expression_fault(&self) -> bool {
        matches!(self, Self::ExpressionEvaluationFault { .. })
    }

    /// Whether this reason is a type fault.
    pub fn is_type_fault(&self) -> bool {
        matches!(self, Self::TypeFault { .. })
    }

    /// Whether this reason is a dependency fault.
    pub fn is_dependency_fault(&self) -> bool {
        matches!(self, Self::DependencyFault { .. })
    }

    /// Whether this reason is a frontier deliberation fault.
    pub fn is_frontier_fault(&self) -> bool {
        matches!(
            self,
            Self::DecisionKeyConflict { .. }
                | Self::AdmissionError { .. }
                | Self::EvaluationError { .. }
                | Self::InvalidPhase { .. }
        )
    }

    /// Whether this reason is a commit or settlement fault.
    pub fn is_commit_fault(&self) -> bool {
        matches!(
            self,
            Self::CommitTickError { .. }
                | Self::QuiescenceVerificationFault { .. }
                | Self::InvariantViolation { .. }
        )
    }
}

impl From<EvaluationFault> for FiniteDecisionUnknownReason {
    fn from(fault: EvaluationFault) -> Self {
        match fault {
            EvaluationFault::KeyConflict(_) | EvaluationFault::CandidateKeyConflict(_) => {
                Self::DecisionKeyConflict {
                    detail: format!("{fault}"),
                }
            }
            EvaluationFault::AdmissionError { candidate, detail } => Self::AdmissionError {
                candidate: candidate.name,
                detail,
            },
            EvaluationFault::EvaluationError { detail } => Self::EvaluationError { detail },
            EvaluationFault::InvalidPhase { candidate, phase } => Self::InvalidPhase {
                candidate: candidate.name,
                phase,
            },
        }
    }
}

impl fmt::Display for FiniteDecisionUnknownReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExpressionEvaluationFault { context, fault } => {
                write!(f, "expression evaluation fault in {context}: {fault:?}")
            }
            Self::DependencyFault { context, detail } => {
                write!(f, "dependency fault in {context}: {detail}")
            }
            Self::TypeFault { context, detail } => {
                write!(f, "type fault in {context}: {detail}")
            }
            Self::DecisionKeyConflict { detail } => {
                write!(f, "decision key conflict under B^uk discipline: {detail}")
            }
            Self::AdmissionError { candidate, detail } => {
                write!(
                    f,
                    "deliberation frontier admission error for candidate '{candidate}': {detail}"
                )
            }
            Self::EvaluationError { detail } => {
                write!(f, "deliberation frontier evaluation error: {detail}")
            }
            Self::InvalidPhase { candidate, phase } => {
                write!(
                    f,
                    "candidate '{candidate}' declared invalid non-zero phase {phase}"
                )
            }
            Self::QuiescenceVerificationFault { detail } => {
                write!(f, "quiescence verification fault: {detail}")
            }
            Self::CommitTickError { detail } => {
                write!(f, "commit tick error: {detail}")
            }
            Self::InvariantViolation { detail } => {
                write!(f, "invariant violation: {detail}")
            }
        }
    }
}

/// Termination status of a finite-decision deliberation run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FiniteDecisionStop {
    /// Exactly one candidate proposal was admitted and selected.
    Selected(SelectedDecision),
    /// All candidates were rejected: successful certified quiescence with verified certificate.
    Quiescent {
        certificate: QuiescenceCertificateId,
    },
    /// Execution or deliberation faulted: returns Unknown and publishes no decision.
    Unknown(FiniteDecisionUnknownReason),
}

/// One commit pool's own outcome within a multi-commit deliberation run
/// (ADR-0039). Every finite-decision module declares one or more commit
/// pools; each deliberates independently over its own candidates — a
/// proposal never competes against a candidate from a different pool — but
/// all pools see the same rules and facts, computed once.
#[derive(Clone, Debug)]
pub struct FiniteDecisionCommitRun {
    pub commit: String,
    pub decision: Option<SelectedDecision>,
    pub dispositions: Vec<CandidateDisposition>,
    pub final_world: ConfigId,
    pub stop: FiniteDecisionStop,
    /// This pool's own committed step, if it selected a candidate. `None`
    /// for certified quiescence or an Unknown fault — nothing is journaled
    /// for a pool that publishes no decision.
    pub step: Option<CommittedStep>,
}

impl FiniteDecisionCommitRun {
    /// Whether this pool selected a candidate.
    pub fn is_selected(&self) -> bool {
        matches!(self.stop, FiniteDecisionStop::Selected(_))
    }

    /// Whether this pool halted in certified quiescence.
    pub fn is_quiescent(&self) -> bool {
        matches!(self.stop, FiniteDecisionStop::Quiescent { .. })
    }

    /// Whether this pool halted with an Unknown fault.
    pub fn is_unknown(&self) -> bool {
        matches!(self.stop, FiniteDecisionStop::Unknown(_))
    }
}

/// The complete report of a finite-decision deliberation run.
///
/// `decision`, `dispositions`, `final_world`, and `stop` mirror the *first*
/// commit pool's own outcome (`commits[0]`, in declaration order) — for the
/// overwhelmingly common single-commit module this is the run's only
/// outcome, so these fields keep exactly their pre-ADR-0039 meaning. A
/// multi-commit module's other pools are reported only in `commits`; use
/// [`FiniteDecisionRun::commit_run`] to look one up by name rather than by
/// position.
#[derive(Clone, Debug)]
pub struct FiniteDecisionRun {
    pub program: FiniteDecisionProgramId,
    pub context: ContextId,
    pub inputs: Vec<BoundInput>,
    pub facts: Vec<DerivedFact>,
    pub decision: Option<SelectedDecision>,
    pub dispositions: Vec<CandidateDisposition>,
    pub journal: Journal,
    pub final_world: ConfigId,
    pub stop: FiniteDecisionStop,
    /// Every commit pool's own outcome, in declaration order (ADR-0039).
    /// `commits[0]` is exactly what `decision`/`dispositions`/`final_world`/
    /// `stop` above report.
    pub commits: Vec<FiniteDecisionCommitRun>,
}

impl FiniteDecisionRun {
    /// Whether a candidate was selected (first commit pool — see the struct docs).
    pub fn is_selected(&self) -> bool {
        matches!(self.stop, FiniteDecisionStop::Selected(_))
    }

    /// Whether deliberation halted in certified quiescence (first commit pool).
    pub fn is_quiescent(&self) -> bool {
        matches!(self.stop, FiniteDecisionStop::Quiescent { .. })
    }

    /// Whether deliberation halted with an Unknown fault (first commit pool).
    pub fn is_unknown(&self) -> bool {
        matches!(self.stop, FiniteDecisionStop::Unknown(_))
    }

    /// Retrieve the structured status disposition of candidate `name`,
    /// searching every commit pool (a candidate belongs to exactly one).
    pub fn status_of(&self, name: &str) -> Option<CandidateStatus> {
        self.commits
            .iter()
            .flat_map(|c| c.dispositions.iter())
            .find(|d| d.name == name)
            .map(|d| d.status.clone())
    }

    /// Look up a bound input record by name.
    pub fn input(&self, name: &str) -> Option<&BoundInput> {
        self.inputs.iter().find(|i| i.name == name)
    }

    /// Look up a commit pool's own outcome by its declared name.
    pub fn commit_run(&self, name: &str) -> Option<&FiniteDecisionCommitRun> {
        self.commits.iter().find(|c| c.commit == name)
    }
}

fn destination_world(program: FiniteDecisionProgramId, proposal: Option<Digest>) -> ConfigId {
    let mut w = CanonWriter::new();
    w.write_bytes(WORLD_MARKER);
    w.write_uint(1);
    w.write_bytes(program.digest().as_bytes());
    match proposal {
        None => w.write_enum(0, |_| {}),
        Some(d) => w.write_enum(1, |w| w.write_bytes(d.as_bytes())),
    }
    ConfigId::from_canon(&w.finish())
}

fn proposal_digest(program: FiniteDecisionProgramId, name: &str) -> Digest {
    let mut w = CanonWriter::new();
    w.write_tag("brix.l3.finite-decision.proposal@1");
    w.write_bytes(program.digest().as_bytes());
    w.write_ident(name);
    w.digest(Domain::Value)
}

/// The per-instance analogue of [`proposal_digest`] for a `decide` block's
/// candidate (ADR-0043): namespaced by the block's own name and the
/// element's index, so the same candidate name reused across two different
/// entities — or across a `decide` block and an unrelated `commit` pool —
/// never collides on the same destination world, generator id, or witness.
fn entity_proposal_digest(
    program: FiniteDecisionProgramId,
    decide: &str,
    index: usize,
    name: &str,
) -> Digest {
    let mut w = CanonWriter::new();
    w.write_tag("brix.l3.finite-decision.entity-proposal@1");
    w.write_bytes(program.digest().as_bytes());
    w.write_ident(decide);
    w.write_uint(index as u64);
    w.write_ident(name);
    w.digest(Domain::Value)
}

fn policy_id(program: FiniteDecisionProgramId) -> ConfigId {
    let mut w = CanonWriter::new();
    w.write_bytes(POLICY_MARKER);
    w.write_uint(1);
    w.write_bytes(program.digest().as_bytes());
    ConfigId::from_canon(&w.finish())
}

fn context_id(
    program: FiniteDecisionProgramId,
    initial: ConfigId,
    policy: ConfigId,
    snapshot: Option<&InputSnapshot>,
) -> ContextId {
    input_context_id(program, initial, policy, snapshot)
}

fn generator_id(
    program: FiniteDecisionProgramId,
    name: &str,
    src: ConfigId,
    dst: ConfigId,
) -> GeneratorId {
    let mut w = CanonWriter::new();
    w.write_tag(GENERATOR_TAG);
    w.write_bytes(program.digest().as_bytes());
    w.write_ident(name);
    w.write_bytes(src.digest().as_bytes());
    w.write_bytes(dst.digest().as_bytes());
    GeneratorId::from_canon(&w.finish())
}

/// The per-instance analogue of [`generator_id`] for a `decide` block's
/// candidate (ADR-0043); see [`entity_proposal_digest`] for the namespacing
/// rationale.
fn entity_generator_id(
    program: FiniteDecisionProgramId,
    decide: &str,
    index: usize,
    name: &str,
    src: ConfigId,
    dst: ConfigId,
) -> GeneratorId {
    let mut w = CanonWriter::new();
    w.write_tag("brix.l3.finite-decision.entity-generator@1");
    w.write_bytes(program.digest().as_bytes());
    w.write_ident(decide);
    w.write_uint(index as u64);
    w.write_ident(name);
    w.write_bytes(src.digest().as_bytes());
    w.write_bytes(dst.digest().as_bytes());
    GeneratorId::from_canon(&w.finish())
}

/// The outcome of one [`FiniteDecisionRuntime::deliberate`] call — a single
/// pool's or a single per-entity instance's own selection-or-quiescence
/// settlement (ADR-0030, ADR-0039, ADR-0043).
struct DeliberationOutcome {
    decision: Option<SelectedDecision>,
    dispositions: Vec<CandidateDisposition>,
    final_world: ConfigId,
    stop: FiniteDecisionStop,
    step: Option<CommittedStep>,
}

#[derive(Clone, Debug)]
struct PresenterEntry {
    name: String,
    candidate: Candidate,
    generator: GeneratorId,
    src: ConfigId,
    dst: ConfigId,
    priority: u64,
    tiebreak: Digest,
}

#[derive(Clone)]
struct CandidatePresenter {
    initial: Handle,
    entries: Vec<PresenterEntry>,
}

impl WitnessProvider for CandidatePresenter {
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        if e.world == self.initial {
            self.entries.iter().map(|entry| entry.candidate).collect()
        } else {
            Vec::new()
        }
    }
}

impl SettlementWitnessProvider for CandidatePresenter {
    fn try_decompose(
        &self,
        e: &ExecConfig,
        candidate: &Candidate,
    ) -> Result<Decomposition, CommitError> {
        if e.world != self.initial {
            return Err(CommitError::UnresolvedHandle);
        }
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.candidate == *candidate)
            .ok_or(CommitError::UnresolvedHandle)?;
        Decomposition::recorded(vec![entry.generator], vec![entry.src, entry.dst])
            .map_err(CommitError::from)
    }
}

/// Runtime coordinator for finite-decision alpha plans.
pub struct FiniteDecisionRuntime {
    pub program: FiniteDecisionProgramId,
    pub context: ContextId,
    pub initial_world: ConfigId,
    pub policy: ConfigId,
    /// Interior-mutable so that per-entity `decide` deliberation (ADR-0043)
    /// can intern the witness/successor handles for candidates discovered
    /// only at run time (one element's candidates per list element), which
    /// a static `commit` pool's candidates never need since every one of
    /// those handles is already interned once, here, at construction.
    interner: std::cell::RefCell<Interner>,
    initial: Handle,
    policy_handle: Handle,
    entries: Vec<PresenterEntry>,
    plan: FiniteDecisionPlan,
    snapshot: InputSnapshot,
    bound_inputs: Vec<BoundInput>,
    functions: Arc<BTreeMap<String, L3FunctionDef>>,
}

impl FiniteDecisionRuntime {
    /// Construct a new runtime from a lowered [`FiniteDecisionPlan`] with no external inputs.
    ///
    /// Fails with [`FiniteDecisionBuildError::InputValidation`] if the plan declares any inputs.
    pub fn build(plan: &FiniteDecisionPlan) -> Result<Self, FiniteDecisionBuildError> {
        Self::build_with_inputs(plan, &InputSnapshot::empty())
    }

    /// Construct a new runtime from a lowered [`FiniteDecisionPlan`] and an [`InputSnapshot`].
    ///
    /// Validates completeness and type agreement against plan input declarations before
    /// computing context identity or initializing the runtime.
    pub fn build_with_inputs(
        plan: &FiniteDecisionPlan,
        snapshot: &InputSnapshot,
    ) -> Result<Self, FiniteDecisionBuildError> {
        snapshot.validate_completeness(plan)?;

        let program = finite_decision_program_id(plan);
        let initial_world = destination_world(program, None);
        let policy = policy_id(program);
        let context = context_id(program, initial_world, policy, Some(snapshot));

        let mut interner = Interner::new();
        let initial = interner.intern(initial_world.digest());
        let policy_handle = interner.intern(policy.digest());

        let regime_id = RegimeId::named(FINITE_FRONTIER_REGIME_NAME);
        let all_candidates: Vec<&String> = plan
            .commits
            .iter()
            .flat_map(|c| c.candidates.iter())
            .collect();
        let mut entries = Vec::with_capacity(all_candidates.len());
        for cand_name in all_candidates {
            let proposal = plan.find_proposal(cand_name).ok_or_else(|| {
                FiniteDecisionBuildError::MissingProposal {
                    candidate: cand_name.clone(),
                }
            })?;
            let prop_digest = proposal_digest(program, cand_name);
            let dst = destination_world(program, Some(prop_digest));
            let generator = generator_id(program, cand_name, initial_world, dst);
            let successor = interner.intern(dst.digest());

            let witness = Witness::new(initial_world, dst, regime_id);
            let witness_handle = interner.intern(witness.id().digest());

            let named = NamedCandidate::with_handles(
                cand_name.clone(),
                regime_id,
                initial_world,
                dst,
                initial,
                witness_handle,
                successor,
                proposal.priority,
            );
            let tiebreak = named.canonical_tiebreak(&interner);

            entries.push(PresenterEntry {
                name: cand_name.clone(),
                candidate: Candidate {
                    witness: witness_handle,
                    successor,
                },
                generator,
                src: initial_world,
                dst,
                priority: proposal.priority,
                tiebreak,
            });
        }

        let mut bound_inputs = Vec::with_capacity(plan.inputs.len());
        for decl in &plan.inputs {
            let val =
                snapshot
                    .get(&decl.name)
                    .ok_or_else(|| InputValidationError::MissingInput {
                        name: decl.name.clone(),
                        declared: decl.ty.clone(),
                    })?;
            bound_inputs.push(BoundInput {
                ordinal: decl.ordinal,
                name: decl.name.clone(),
                ty: decl.ty.clone(),
                value: val.to_l3_value(),
                grade: Outcome::Derived,
            });
        }

        let schema_table = Arc::new(plan.schemas.clone());
        let mut functions_map = BTreeMap::new();
        for f in &plan.functions {
            let params = f
                .params
                .iter()
                .map(|p| {
                    (
                        p.name.clone(),
                        p.contract.as_ref().map(|c| {
                            c.schema_ty.clone().unwrap_or_else(|| match c.ty {
                                L3ValueType::Int => L3SchemaType::Int,
                                L3ValueType::Bool => L3SchemaType::Bool,
                                L3ValueType::Str => L3SchemaType::Str,
                                L3ValueType::Sum(ref name) | L3ValueType::Record(ref name) => {
                                    L3SchemaType::Named(name.clone())
                                }
                                // Helper contracts never carry `List`
                                // (ADR-0037 §Scope, ADR-0040): `parse_contract`
                                // rejects `List<T>` as an unsupported contract
                                // type before a `FiniteDecisionContract` is
                                // ever built.
                                L3ValueType::List => {
                                    unreachable!("helper contracts never carry a List value type")
                                }
                            })
                        }),
                    )
                })
                .collect();
            let ret_contract = f.ret_contract.as_ref().map(|c| {
                c.schema_ty.clone().unwrap_or_else(|| match c.ty {
                    L3ValueType::Int => L3SchemaType::Int,
                    L3ValueType::Bool => L3SchemaType::Bool,
                    L3ValueType::Str => L3SchemaType::Str,
                    L3ValueType::Sum(ref name) | L3ValueType::Record(ref name) => {
                        L3SchemaType::Named(name.clone())
                    }
                    L3ValueType::List => {
                        unreachable!("helper contracts never carry a List value type")
                    }
                })
            });
            functions_map.insert(
                f.name.clone(),
                L3FunctionDef {
                    name: f.name.clone(),
                    params,
                    ret_contract,
                    body: f.body.clone(),
                    schemas: schema_table.clone(),
                },
            );
        }
        let functions = Arc::new(functions_map);

        Ok(Self {
            program,
            context,
            initial_world,
            policy,
            interner: std::cell::RefCell::new(interner),
            initial,
            policy_handle,
            entries,
            plan: plan.clone(),
            snapshot: snapshot.clone(),
            bound_inputs,
            functions,
        })
    }

    /// The validated input snapshot bound into this runtime.
    pub fn snapshot(&self) -> &InputSnapshot {
        &self.snapshot
    }

    /// The bound input records in declaration order.
    pub fn bound_inputs(&self) -> &[BoundInput] {
        &self.bound_inputs
    }

    /// The helper function table bound into this runtime.
    pub fn functions(&self) -> &Arc<BTreeMap<String, L3FunctionDef>> {
        &self.functions
    }

    /// Initial execution configuration for this runtime.
    pub fn initial_exec(&self) -> ExecConfig {
        ExecConfig::new(self.initial, self.policy_handle, History::empty().digest())
    }

    /// Initial candidates registered in this runtime (for coexistence checks).
    pub fn candidates_at_initial(&self) -> Vec<(String, u64, ConfigId)> {
        self.entries
            .iter()
            .map(|e| (e.name.clone(), e.priority, e.dst))
            .collect()
    }

    /// Derive the generator registry and semantics for this runtime.
    pub fn audit_environment(&self) -> (ContextId, GeneratorRegistry, GeneratorSemanticsV1) {
        let mut registry = GeneratorRegistry::new();
        let mut semantics = GeneratorSemanticsV1::new();
        for entry in &self.entries {
            registry.insert(entry.generator);
            semantics.declare_rows(entry.generator, [(entry.src, entry.dst)]);
        }
        (self.context, registry, semantics)
    }

    /// Audit a committed journal against this runtime's generator semantics.
    pub fn audit(&self, journal: &Journal) -> Vec<AuditResult> {
        let (context, registry, semantics) = self.audit_environment();
        audit_journal(journal, context, &registry, &semantics)
    }

    /// Execute the complete finite-decision deliberation cycle.
    pub fn run(&self) -> FiniteDecisionRun {
        // Step 0: Inject functions and bound inputs into evaluation environment before lets/rules/proposals.
        let mut env = EvalEnv::new()
            .with_functions(self.functions.clone())
            .with_schemas(Arc::new(self.plan.schemas.clone()));
        for input in &self.bound_inputs {
            env = env.with_input(input.name.clone(), input.value.clone());
        }

        // Every declared commit pool faults identically when a shared stage
        // (lets or rules, below) faults before any pool-specific evaluation
        // runs — none of them ever got far enough to differ.
        let shared_stage_fault =
            |facts: Vec<DerivedFact>, reason: FiniteDecisionUnknownReason| -> FiniteDecisionRun {
                let commits: Vec<FiniteDecisionCommitRun> = self
                    .plan
                    .commits
                    .iter()
                    .map(|pool| FiniteDecisionCommitRun {
                        commit: pool.name.clone(),
                        decision: None,
                        dispositions: Vec::new(),
                        final_world: self.initial_world,
                        stop: FiniteDecisionStop::Unknown(reason.clone()),
                        step: None,
                    })
                    .collect();
                FiniteDecisionRun {
                    program: self.program,
                    context: self.context,
                    inputs: self.bound_inputs.clone(),
                    facts,
                    decision: None,
                    dispositions: Vec::new(),
                    journal: Journal::new(),
                    final_world: self.initial_world,
                    stop: FiniteDecisionStop::Unknown(reason),
                    commits,
                }
            };

        // Step 1: Evaluate closed let bindings.
        for (name, expr) in &self.plan.lets {
            match eval(expr, &env) {
                Ok(v) => env = env.with_let(name.clone(), v),
                Err(fault) => {
                    return shared_stage_fault(
                        Vec::new(),
                        FiniteDecisionUnknownReason::ExpressionEvaluationFault {
                            context: format!("let {name}"),
                            fault,
                        },
                    );
                }
            }
        }

        // Step 2: All rules fully evaluate before deliberation.
        let mut facts = Vec::new();
        for rule in &self.plan.rules {
            match eval(&rule.body, &env) {
                Ok(v) => {
                    facts.push(DerivedFact {
                        rule: rule.name.clone(),
                        value: v.clone(),
                        ordinal: rule.ordinal,
                        grade: Outcome::Derived,
                    });
                    env = env.with_fact(rule.name.clone(), v);
                }
                Err(fault) => {
                    return shared_stage_fault(
                        facts,
                        FiniteDecisionUnknownReason::ExpressionEvaluationFault {
                            context: format!("rule {}", rule.name),
                            fault,
                        },
                    );
                }
            }
        }

        // Steps 3-6 deliberate each commit pool independently (ADR-0039): a
        // candidate never competes against a candidate from another pool,
        // but every pool sees the rules/lets/facts computed once above.
        let commits: Vec<FiniteDecisionCommitRun> = self
            .plan
            .commits
            .iter()
            .map(|pool| self.run_pool(pool, &env))
            .collect();

        // Journal every pool's own step (if any), in commit declaration order.
        let mut journal = Journal::new();
        for c in &commits {
            if let Some(step) = c.step.clone() {
                journal.append(step);
            }
        }

        // `decision`/`dispositions`/`final_world`/`stop` mirror the first
        // commit pool (see the struct docs) — lowering guarantees at least
        // one commit, so this is always present.
        let first = commits
            .first()
            .expect("finite-decision plan always has at least one commit pool");
        let decision = first.decision.clone();
        let dispositions = first.dispositions.clone();
        let final_world = first.final_world;
        let stop = first.stop.clone();

        FiniteDecisionRun {
            program: self.program,
            context: self.context,
            inputs: self.bound_inputs.clone(),
            facts,
            decision,
            dispositions,
            journal,
            final_world,
            stop,
            commits,
        }
    }

    /// Deliberate a single commit pool: evaluate its candidates' guards and
    /// values, run the deliberation frontier restricted to just this pool's
    /// entries, and settle its own commit tick or certified quiescence. This
    /// is exactly what `run()` did for the (formerly sole) commit pool
    /// before ADR-0039; it is now called once per declared pool.
    fn run_pool(&self, pool: &FiniteDecisionCommit, env: &EvalEnv) -> FiniteDecisionCommitRun {
        let fault = |reason: FiniteDecisionUnknownReason,
                     dispositions: Vec<CandidateDisposition>|
         -> FiniteDecisionCommitRun {
            FiniteDecisionCommitRun {
                commit: pool.name.clone(),
                decision: None,
                dispositions,
                final_world: self.initial_world,
                stop: FiniteDecisionStop::Unknown(reason),
                step: None,
            }
        };

        // Step 3: Evaluate proposal guards and values in this commit pool.
        let mut admitted_names = BTreeSet::new();
        let mut candidate_values = Vec::new();

        for cand_name in &pool.candidates {
            let Some(proposal) = self.plan.find_proposal(cand_name) else {
                return fault(
                    FiniteDecisionUnknownReason::DependencyFault {
                        context: format!("candidate proposal {cand_name}"),
                        detail: "proposal missing from plan".to_string(),
                    },
                    Vec::new(),
                );
            };

            // Evaluate guard: must be Bool.
            match eval(&proposal.guard, env) {
                Ok(L3ValueV2::Bool(true)) => {
                    admitted_names.insert(cand_name.clone());
                }
                Ok(L3ValueV2::Bool(false)) => {}
                Ok(other) => {
                    return fault(
                        FiniteDecisionUnknownReason::TypeFault {
                            context: format!("proposal {} guard", proposal.name),
                            detail: format!(
                                "guard must evaluate to Bool, found {}",
                                type_of_value(&other)
                            ),
                        },
                        Vec::new(),
                    );
                }
                Err(eval_fault) => {
                    return fault(
                        FiniteDecisionUnknownReason::ExpressionEvaluationFault {
                            context: format!("proposal {} guard", proposal.name),
                            fault: eval_fault,
                        },
                        Vec::new(),
                    );
                }
            }

            // Evaluate value.
            match eval(&proposal.value, env) {
                Ok(v) => candidate_values.push((cand_name.clone(), v)),
                Err(eval_fault) => {
                    return fault(
                        FiniteDecisionUnknownReason::ExpressionEvaluationFault {
                            context: format!("proposal {} value", proposal.name),
                            fault: eval_fault,
                        },
                        Vec::new(),
                    );
                }
            }
        }

        // Step 4: All proposal values in this pool must share one type.
        if candidate_values.len() > 1 {
            let expected_type = type_of_value(&candidate_values[0].1);
            for (name, val) in &candidate_values[1..] {
                let actual_type = type_of_value(val);
                if actual_type != expected_type {
                    return fault(
                        FiniteDecisionUnknownReason::TypeFault {
                            context: "proposal values".to_string(),
                            detail: format!(
                                "proposal '{name}' value type {actual_type} does not match expected {expected_type}"
                            ),
                        },
                        Vec::new(),
                    );
                }
            }
        }

        // Steps 5-6: shared with per-entity deliberation (ADR-0043) — see
        // `Self::deliberate`.
        let pool_entries: Vec<PresenterEntry> = self
            .entries
            .iter()
            .filter(|e| pool.candidates.iter().any(|c| c == &e.name))
            .cloned()
            .collect();
        match self.deliberate(&pool.candidates, &pool_entries, &admitted_names, candidate_values, 0) {
            Ok(out) => FiniteDecisionCommitRun {
                commit: pool.name.clone(),
                decision: out.decision,
                dispositions: out.dispositions,
                final_world: out.final_world,
                stop: out.stop,
                step: out.step,
            },
            Err((reason, dispositions)) => fault(reason, dispositions),
        }
    }

    /// Build the [`PresenterEntry`] set for one per-entity `decide` instance
    /// (ADR-0043): candidate `name`s are namespaced by `(decide name,
    /// element index)` in every identity-bearing digest, so two different
    /// instances — or an instance and a top-level `commit` pool — that
    /// happen to declare a same-spelled candidate name never collide on the
    /// same destination world, generator id, or witness.
    fn entity_entries(
        &self,
        decide_name: &str,
        index: usize,
        proposals: &[crate::finite_decision::plan::FiniteDecisionProposal],
    ) -> Vec<PresenterEntry> {
        let regime_id = RegimeId::named(FINITE_FRONTIER_REGIME_NAME);
        let mut interner = self.interner.borrow_mut();
        let mut entries = Vec::with_capacity(proposals.len());
        for proposal in proposals {
            let prop_digest =
                entity_proposal_digest(self.program, decide_name, index, &proposal.name);
            let dst = destination_world(self.program, Some(prop_digest));
            let generator = entity_generator_id(
                self.program,
                decide_name,
                index,
                &proposal.name,
                self.initial_world,
                dst,
            );
            let successor = interner.intern(dst.digest());
            let witness = Witness::new(self.initial_world, dst, regime_id);
            let witness_handle = interner.intern(witness.id().digest());
            let named = NamedCandidate::with_handles(
                proposal.name.clone(),
                regime_id,
                self.initial_world,
                dst,
                self.initial,
                witness_handle,
                successor,
                proposal.priority,
            );
            let tiebreak = named.canonical_tiebreak(&interner);
            entries.push(PresenterEntry {
                name: proposal.name.clone(),
                candidate: Candidate {
                    witness: witness_handle,
                    successor,
                },
                generator,
                src: self.initial_world,
                dst,
                priority: proposal.priority,
                tiebreak,
            });
        }
        entries
    }

    /// The shared core of Steps 5-6 (ADR-0030 §Deliberation): build
    /// [`NamedCandidate`]s from `entries`, evaluate the deliberation
    /// frontier under `admitted_names`, and settle either a single
    /// selection or certified quiescence — used identically by a top-level
    /// `commit` pool ([`Self::run_pool`]) and by one per-entity `decide`
    /// instance (ADR-0043). `phase` is threaded into every calendar [`Key`]
    /// this deliberation computes, purely for cross-run traceability: two
    /// deliberations never share a destination world (every entry's `dst`
    /// already differs — see [`entity_proposal_digest`]), so it carries no
    /// correctness weight, but every pool and every entity instance gets its
    /// own value regardless.
    fn deliberate(
        &self,
        candidate_names: &[String],
        entries: &[PresenterEntry],
        admitted_names: &BTreeSet<String>,
        candidate_values: Vec<(String, L3ValueV2)>,
        phase: u64,
    ) -> Result<DeliberationOutcome, (FiniteDecisionUnknownReason, Vec<CandidateDisposition>)> {
        let interner = self.interner.borrow();
        let regime_id = RegimeId::named(FINITE_FRONTIER_REGIME_NAME);
        let named_candidates: Vec<NamedCandidate> = entries
            .iter()
            .map(|e| {
                NamedCandidate::with_handles(
                    e.name.clone(),
                    regime_id,
                    e.src,
                    e.dst,
                    self.initial,
                    e.candidate.witness,
                    e.candidate.successor,
                    e.priority,
                )
            })
            .collect();

        let policy = FnPolicy(|_e: &ExecConfig, c: &NamedCandidate| {
            if admitted_names.contains(&c.name) {
                soc_regimes::finite_frontier::AdmissionDecision::Admitted
            } else {
                soc_regimes::finite_frontier::AdmissionDecision::rejected_guard_false()
            }
        });

        let exec = self.initial_exec();
        let evaluated = EvaluatedFrontier::evaluate(
            named_candidates.iter().cloned(),
            &policy,
            &exec,
            &interner,
        );

        // Fail-closed on frontier deliberation fault under B^uk discipline.
        if let Some(frontier_fault) = evaluated.fault() {
            return Err((FiniteDecisionUnknownReason::from(frontier_fault), Vec::new()));
        }

        // Compute structured dispositions for all candidates.
        let mut dispositions = Vec::new();
        for cand_name in candidate_names {
            let Some(nc) = named_candidates.iter().find(|c| &c.name == cand_name) else {
                return Err((
                    FiniteDecisionUnknownReason::EvaluationError {
                        detail: format!("candidate '{cand_name}' missing from named candidates"),
                    },
                    Vec::new(),
                ));
            };
            let Some(status) = evaluated.status_of(nc) else {
                return Err((
                    FiniteDecisionUnknownReason::EvaluationError {
                        detail: format!("candidate status missing for '{cand_name}'"),
                    },
                    Vec::new(),
                ));
            };
            dispositions.push(CandidateDisposition {
                name: cand_name.clone(),
                priority: nc.priority,
                status,
            });
        }

        // Step 6: Selection or Certified Quiescence.
        if evaluated.is_quiescent() {
            let entries_owned: Vec<PresenterEntry> = entries.iter().map(|e| (*e).clone()).collect();
            let all_presenter = CandidatePresenter {
                initial: self.initial,
                entries: entries_owned.clone(),
            };
            let adm_adapter = PolicyToAdmAdapter::new(&policy, named_candidates.iter().cloned());
            let all_generators: BTreeSet<GeneratorId> =
                entries.iter().map(|e| e.generator).collect();
            let obs_profile = match GeneratorPartitionProfile::new(all_generators, BTreeSet::new())
            {
                Ok(p) => p,
                Err(err) => {
                    return Err((
                        FiniteDecisionUnknownReason::QuiescenceVerificationFault {
                            detail: format!("invalid observation profile: {err:?}"),
                        },
                        dispositions,
                    ));
                }
            };
            let pres = PresentationV1 {
                id: PresentationIdV1::from_canon(self.program.digest().as_bytes()),
                regimes: &[&all_presenter],
                regime_set: Digest::of(Domain::Value, b"brix.l3.finite-decision.regime-set@1"),
                adm: &adm_adapter,
                adm_id: Digest::of(Domain::Value, b"brix.l3.finite-decision.adm@1"),
                profile: &obs_profile,
                interner: &interner,
                context: self.context,
                assumptions: DeclaredAssumptions::all(),
            };
            let mut k = |c: &Candidate, phase: u64| {
                if let Some(entry) = entries_owned.iter().find(|e| e.candidate == *c) {
                    Key::new(phase, entry.priority, entry.tiebreak)
                } else {
                    Key::new(
                        phase,
                        u64::MAX,
                        Digest::of(Domain::Value, b"missing-candidate"),
                    )
                }
            };
            let (step, _, _) = sat_step(&pres, &exec, phase, &mut k, SaturationBudget::uniform(32));
            let certificate = match step {
                SaturatedStep::Quiescent(cert) => {
                    let cert_id = quiescence_certificate_id(&cert);
                    let check = check_quiescence_certificate(&cert, &pres, &exec, &[]);
                    if !matches!(check, CertificateCheck::Verified { .. }) {
                        return Err((
                            FiniteDecisionUnknownReason::QuiescenceVerificationFault {
                                detail: format!(
                                    "quiescence certificate verification failed: {check:?}"
                                ),
                            },
                            dispositions,
                        ));
                    }
                    cert_id
                }
                other => {
                    return Err((
                        FiniteDecisionUnknownReason::QuiescenceVerificationFault {
                            detail: format!(
                                "saturation did not return quiescence certificate: {other:?}"
                            ),
                        },
                        dispositions,
                    ));
                }
            };

            Ok(DeliberationOutcome {
                decision: None,
                dispositions,
                final_world: self.initial_world,
                stop: FiniteDecisionStop::Quiescent { certificate },
                step: None,
            })
        } else {
            let Some((_, winning_cand)) = evaluated.selected.as_ref() else {
                return Err((
                    FiniteDecisionUnknownReason::EvaluationError {
                        detail: "expected selected candidate in evaluated frontier".to_string(),
                    },
                    dispositions,
                ));
            };
            let admitted_entries: Vec<PresenterEntry> = entries
                .iter()
                .filter(|e| admitted_names.contains(&e.name))
                .map(|e| (*e).clone())
                .collect();
            let admitted_presenter = CandidatePresenter {
                initial: self.initial,
                entries: admitted_entries.clone(),
            };
            let mut keyer = |c: &Candidate, phase: u64| {
                if let Some(entry) = admitted_entries.iter().find(|e| e.candidate == *c) {
                    Key::new(phase, entry.priority, entry.tiebreak)
                } else {
                    Key::new(
                        phase,
                        u64::MAX,
                        Digest::of(Domain::Value, b"missing-candidate"),
                    )
                }
            };
            let tick_res = try_commit_tick(
                &[&admitted_presenter],
                &AdmAll,
                &interner,
                &exec,
                self.context,
                phase,
                &mut keyer,
            );

            let (committed, step, _) = match tick_res {
                Ok(triple) => triple,
                Err(err) => {
                    return Err((
                        FiniteDecisionUnknownReason::CommitTickError {
                            detail: format!("{err:?}"),
                        },
                        dispositions,
                    ));
                }
            };

            let Committed::Step { observation, .. } = committed else {
                return Err((
                    FiniteDecisionUnknownReason::CommitTickError {
                        detail: "expected committed step".to_string(),
                    },
                    dispositions,
                ));
            };

            // Contract: Results are Derived, never Proven or Refuted.
            if observation.outcome_class != Outcome::Derived {
                return Err((
                    FiniteDecisionUnknownReason::CommitTickError {
                        detail: format!(
                            "committed observation grade {:?} is not Derived",
                            observation.outcome_class
                        ),
                    },
                    dispositions,
                ));
            }

            let Some(step) = step else {
                return Err((
                    FiniteDecisionUnknownReason::CommitTickError {
                        detail: "Committed::Step missing journal record".to_string(),
                    },
                    dispositions,
                ));
            };

            let winning_val = match candidate_values
                .into_iter()
                .find(|(n, _)| n == &winning_cand.name)
            {
                Some((_, val)) => val,
                None => {
                    return Err((
                        FiniteDecisionUnknownReason::EvaluationError {
                            detail: format!(
                                "winning candidate '{}' value missing from evaluated candidate values",
                                winning_cand.name
                            ),
                        },
                        dispositions,
                    ));
                }
            };

            let decision = SelectedDecision {
                candidate: winning_cand.name.clone(),
                priority: winning_cand.priority,
                value: winning_val,
                grade: Outcome::Derived,
            };

            Ok(DeliberationOutcome {
                decision: Some(decision.clone()),
                dispositions,
                final_world: winning_cand.dst,
                stop: FiniteDecisionStop::Selected(decision),
                step: Some(step),
            })
        }
    }

    /// Re-derive why a candidate was admitted/selected fresh from inputs.
    pub fn explain_why(
        &self,
        target_name: &str,
    ) -> Result<WhyExplanation, FiniteDecisionUnknownReason> {
        let run = self.run();
        let pool = self.plan.commit_of_candidate(target_name);
        // Fail-closed if the relevant pool's deliberation faulted (ADR-0039:
        // scoped to `target_name`'s own pool; for a target that names no
        // candidate in any pool, this falls back to the first pool's stop,
        // exactly matching pre-ADR-0039 behavior on a single-commit plan).
        let stop_to_check = match pool {
            Some(p) => run.commit_run(&p.name).map(|r| r.stop.clone()),
            None => Some(run.stop.clone()),
        };
        if let Some(FiniteDecisionStop::Unknown(reason)) = stop_to_check {
            return Err(reason);
        }
        let Some(pool) = pool else {
            return Ok(WhyExplanation::CandidateNotFound);
        };
        let pool_run = run.commit_run(&pool.name);

        let regime_id = RegimeId::named(FINITE_FRONTIER_REGIME_NAME);
        let named_candidates: Vec<NamedCandidate> = self
            .entries
            .iter()
            .filter(|e| pool.candidates.iter().any(|c| c == &e.name))
            .map(|e| {
                NamedCandidate::with_handles(
                    e.name.clone(),
                    regime_id,
                    e.src,
                    e.dst,
                    self.initial,
                    e.candidate.witness,
                    e.candidate.successor,
                    e.priority,
                )
            })
            .collect();

        let Some(target) = named_candidates.iter().find(|c| c.name == target_name) else {
            return Ok(WhyExplanation::CandidateNotFound);
        };

        let admitted_names: BTreeSet<String> = pool_run
            .map(|r| r.dispositions.as_slice())
            .unwrap_or(&[])
            .iter()
            .filter(|d| d.status != CandidateStatus::RejectedGuardFalse)
            .map(|d| d.name.clone())
            .collect();
        let policy = FnPolicy(|_e: &ExecConfig, c: &NamedCandidate| {
            if admitted_names.contains(&c.name) {
                soc_regimes::finite_frontier::AdmissionDecision::Admitted
            } else {
                soc_regimes::finite_frontier::AdmissionDecision::rejected_guard_false()
            }
        });
        let interner = self.interner.borrow();
        let explanation = explain_why(
            &named_candidates,
            &policy,
            &self.initial_exec(),
            target,
            &interner,
        );
        if let WhyExplanation::EvaluationFaulted { fault, .. } = &explanation {
            return Err(FiniteDecisionUnknownReason::from(fault.clone()));
        }
        Ok(explanation)
    }

    /// Re-derive why a candidate was NOT admitted or NOT selected fresh from inputs.
    pub fn explain_why_not(
        &self,
        target_name: &str,
    ) -> Result<WhyNotExplanation, FiniteDecisionUnknownReason> {
        let run = self.run();
        let pool = self.plan.commit_of_candidate(target_name);
        let stop_to_check = match pool {
            Some(p) => run.commit_run(&p.name).map(|r| r.stop.clone()),
            None => Some(run.stop.clone()),
        };
        if let Some(FiniteDecisionStop::Unknown(reason)) = stop_to_check {
            return Err(reason);
        }
        let Some(pool) = pool else {
            return Ok(WhyNotExplanation::CandidateNotFound);
        };
        let pool_run = run.commit_run(&pool.name);

        let regime_id = RegimeId::named(FINITE_FRONTIER_REGIME_NAME);
        let named_candidates: Vec<NamedCandidate> = self
            .entries
            .iter()
            .filter(|e| pool.candidates.iter().any(|c| c == &e.name))
            .map(|e| {
                NamedCandidate::with_handles(
                    e.name.clone(),
                    regime_id,
                    e.src,
                    e.dst,
                    self.initial,
                    e.candidate.witness,
                    e.candidate.successor,
                    e.priority,
                )
            })
            .collect();

        let Some(target) = named_candidates.iter().find(|c| c.name == target_name) else {
            return Ok(WhyNotExplanation::CandidateNotFound);
        };

        let admitted_names: BTreeSet<String> = pool_run
            .map(|r| r.dispositions.as_slice())
            .unwrap_or(&[])
            .iter()
            .filter(|d| d.status != CandidateStatus::RejectedGuardFalse)
            .map(|d| d.name.clone())
            .collect();
        let policy = FnPolicy(|_e: &ExecConfig, c: &NamedCandidate| {
            if admitted_names.contains(&c.name) {
                soc_regimes::finite_frontier::AdmissionDecision::Admitted
            } else {
                soc_regimes::finite_frontier::AdmissionDecision::rejected_guard_false()
            }
        });
        let interner = self.interner.borrow();
        let explanation = explain_why_not(
            &named_candidates,
            &policy,
            &self.initial_exec(),
            target,
            &interner,
        );
        if let WhyNotExplanation::EvaluationFaulted { fault, .. } = &explanation {
            return Err(FiniteDecisionUnknownReason::from(fault.clone()));
        }
        Ok(explanation)
    }

    /// Build a structured, bounded derivation explanation for `target_name`:
    /// the admission guard's evaluation trace, every rule/`let`/input it
    /// transitively reads, the proposal's value trace, and — for an admitted
    /// candidate — the calendar comparison against the actual winner.
    ///
    /// Purely informational (ADR-0030): this reuses [`Self::explain_why`]'s
    /// own `Key` comparison rather than restating the calendar's ordering,
    /// and every value in the trace comes from re-evaluating the exact
    /// subexpression with [`crate::l3_v2::eval`] over the same environment
    /// `run()` folded inputs, lets, and facts into — so it cannot disagree
    /// with `run()`'s own result, and never changes it, the run's
    /// program/context/snapshot identity, or any grade.
    pub fn explain_candidate(
        &self,
        target_name: &str,
    ) -> Result<crate::finite_decision::explain::ExplainOutcome, FiniteDecisionUnknownReason> {
        use crate::finite_decision::explain;

        let run = self.run();
        if let FiniteDecisionStop::Unknown(reason) = &run.stop {
            return Err(reason.clone());
        }

        let why = self.explain_why(target_name)?;
        if matches!(why, WhyExplanation::CandidateNotFound) {
            return Ok(explain::ExplainOutcome::CandidateNotFound);
        }
        let selection = explain::selection_from_why(&why);

        let mut env = EvalEnv::new()
            .with_functions(self.functions.clone())
            .with_schemas(Arc::new(self.plan.schemas.clone()));
        for input in &self.bound_inputs {
            env = env.with_input(input.name.clone(), input.value.clone());
        }
        for (name, expr) in &self.plan.lets {
            match eval(expr, &env) {
                Ok(v) => env = env.with_let(name.clone(), v),
                Err(fault) => {
                    // `run` already succeeded above, so every `let` already
                    // evaluated cleanly over this same construction; this is
                    // unreachable on a consistent plan, and failing closed
                    // here is strictly safer than assuming it never happens.
                    return Err(FiniteDecisionUnknownReason::ExpressionEvaluationFault {
                        context: format!("let {name}"),
                        fault,
                    });
                }
            }
        }
        for fact in &run.facts {
            env = env.with_fact(fact.rule.clone(), fact.value.clone());
        }

        Ok(explain::explain_candidate(
            &self.plan,
            &run,
            &env,
            target_name,
            selection,
        ))
    }

    /// Re-evaluate all declared show expressions against the runtime's bound inputs, lets, and derived facts.
    pub fn evaluate_shows(
        &self,
        run: &FiniteDecisionRun,
    ) -> Result<Vec<L3ValueV2>, FiniteDecisionUnknownReason> {
        self.evaluate_shows_exprs(run, &self.plan.shows)
    }

    /// Re-evaluate an explicit list of lowered show expressions against this
    /// runtime's bound inputs, lets, and derived facts.
    ///
    /// This runtime's own `plan` is used only for its lets/facts/functions —
    /// `shows` need not be `self.plan.shows` (see [`Self::evaluate_shows`]).
    /// This is how `brix run` prints `show` results without `show` items
    /// feeding the canonical program identity: the plan a runtime is *built*
    /// from is lowered from a `show`-free module (so `self.program` never
    /// depends on whether the source declares any `show`), while the show
    /// expressions actually printed are lowered separately, from the full
    /// module, and passed here.
    pub fn evaluate_shows_exprs(
        &self,
        run: &FiniteDecisionRun,
        shows: &[L3ExprV2],
    ) -> Result<Vec<L3ValueV2>, FiniteDecisionUnknownReason> {
        if run.program != self.program {
            return Err(FiniteDecisionUnknownReason::InvariantViolation {
                detail: format!(
                    "program mismatch in evaluate_shows: expected {:?}, found {:?}",
                    self.program, run.program
                ),
            });
        }
        if run.context != self.context {
            return Err(FiniteDecisionUnknownReason::InvariantViolation {
                detail: format!(
                    "context mismatch in evaluate_shows: expected {}, found {}",
                    self.context.digest().to_hex(),
                    run.context.digest().to_hex()
                ),
            });
        }
        if run.inputs != self.bound_inputs {
            return Err(FiniteDecisionUnknownReason::InvariantViolation {
                detail: "bound inputs mismatch in evaluate_shows".to_string(),
            });
        }

        // Strict integrity check: re-derive deliberation facts from the runtime and assert caller run integrity.
        let fresh_run = self.run();
        if run.facts != fresh_run.facts {
            return Err(FiniteDecisionUnknownReason::InvariantViolation {
                detail: "derived facts mismatch in evaluate_shows: supplied run facts do not match deterministic evaluation".to_string(),
            });
        }
        if run.stop != fresh_run.stop {
            return Err(FiniteDecisionUnknownReason::InvariantViolation {
                detail: "deliberation stop mismatch in evaluate_shows: supplied run stop condition does not match deterministic evaluation".to_string(),
            });
        }

        let mut env = EvalEnv::new()
            .with_functions(self.functions.clone())
            .with_schemas(Arc::new(self.plan.schemas.clone()));
        for input in &self.bound_inputs {
            env = env.with_input(input.name.clone(), input.value.clone());
        }
        for (name, expr) in &self.plan.lets {
            match eval(expr, &env) {
                Ok(v) => env = env.with_let(name.clone(), v),
                Err(fault) => {
                    return Err(FiniteDecisionUnknownReason::ExpressionEvaluationFault {
                        context: format!("let {name}"),
                        fault,
                    });
                }
            }
        }
        for fact in &fresh_run.facts {
            env = env.with_fact(fact.rule.clone(), fact.value.clone());
        }
        // Each commit pool's own name is a "committed fact" too
        // (ast::Item::Show's doc comment: "surfaces a committed fact") —
        // bound only when that pool actually selected a candidate, so
        // `show <commit name>` for a quiescent (or unevaluated) pool is
        // unbound (a fault, not a fabricated value). ADR-0039: every pool
        // binds its own name independently.
        for commit_run in &fresh_run.commits {
            if let Some(decision) = &commit_run.decision {
                env = env.with_fact(commit_run.commit.clone(), decision.value.clone());
            }
        }
        let mut results = Vec::with_capacity(shows.len());
        for (idx, show_expr) in shows.iter().enumerate() {
            match eval(show_expr, &env) {
                Ok(v) => results.push(v),
                Err(fault) => {
                    return Err(FiniteDecisionUnknownReason::ExpressionEvaluationFault {
                        context: format!("show[{idx}]"),
                        fault,
                    });
                }
            }
        }
        Ok(results)
    }
}

/// Run a finite-decision plan with no external inputs through deliberation to completion.
///
/// Fails if the plan declares any inputs (ADR-0031).
pub fn run_finite_decision_plan(
    plan: &FiniteDecisionPlan,
) -> Result<FiniteDecisionRun, FiniteDecisionBuildError> {
    let runtime = FiniteDecisionRuntime::build(plan)?;
    Ok(runtime.run())
}

/// Run a finite-decision plan with an external input snapshot through deliberation to completion.
pub fn run_finite_decision_plan_with_inputs(
    plan: &FiniteDecisionPlan,
    snapshot: &InputSnapshot,
) -> Result<FiniteDecisionRun, FiniteDecisionBuildError> {
    let runtime = FiniteDecisionRuntime::build_with_inputs(plan, snapshot)?;
    Ok(runtime.run())
}

/// Derive the run context, generator registry, and generator semantics for a finite-decision plan with no inputs.
///
/// Refuses input-declaring plans fail-closed (ADR-0031).
pub fn finite_decision_audit_environment_from_plan(
    plan: &FiniteDecisionPlan,
) -> Result<(ContextId, GeneratorRegistry, GeneratorSemanticsV1), FiniteDecisionBuildError> {
    finite_decision_audit_environment_from_plan_with_inputs(plan, &InputSnapshot::empty())
}

/// Derive the run context, generator registry, and generator semantics for a finite-decision plan with an input snapshot.
pub fn finite_decision_audit_environment_from_plan_with_inputs(
    plan: &FiniteDecisionPlan,
    snapshot: &InputSnapshot,
) -> Result<(ContextId, GeneratorRegistry, GeneratorSemanticsV1), FiniteDecisionBuildError> {
    snapshot.validate_completeness(plan)?;

    let program = finite_decision_program_id(plan);
    let initial_world = destination_world(program, None);
    let policy = policy_id(program);
    let context = context_id(program, initial_world, policy, Some(snapshot));

    let mut registry = GeneratorRegistry::new();
    let mut semantics = GeneratorSemanticsV1::new();
    for cand_name in plan.commits.iter().flat_map(|c| c.candidates.iter()) {
        let _proposal = plan.find_proposal(cand_name).ok_or_else(|| {
            FiniteDecisionBuildError::MissingProposal {
                candidate: cand_name.clone(),
            }
        })?;
        let prop_digest = proposal_digest(program, cand_name);
        let dst = destination_world(program, Some(prop_digest));
        let generator = generator_id(program, cand_name, initial_world, dst);
        registry.insert(generator);
        semantics.declare_rows(generator, [(initial_world, dst)]);
    }
    Ok((context, registry, semantics))
}
