//! Finite-decision alpha deliberation runtime and settlement integration (ADR-0030).

use std::collections::BTreeSet;
use std::fmt;

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
use soc_core::journal::Journal;
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
    finite_decision_program_id, FiniteDecisionPlan, FiniteDecisionProgramId,
};
use crate::input::{input_context_id, InputSnapshot, InputValidationError};
use crate::l3_v2::{eval, EvalEnv, EvalFault, L3ValueV2};

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

impl From<InputValidationError> for FiniteDecisionBuildError {
    fn from(err: InputValidationError) -> Self {
        Self::InputValidation(err)
    }
}

/// Type category of an [`L3ValueV2`] for contract uniformity checking.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum L3ValueType {
    Int,
    Str,
    Bool,
    Sum(String),
    Record(String),
}

impl fmt::Display for L3ValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int => write!(f, "Int"),
            Self::Str => write!(f, "Str"),
            Self::Bool => write!(f, "Bool"),
            Self::Sum(s) => write!(f, "Sum({s})"),
            Self::Record(r) => write!(f, "Record({r})"),
        }
    }
}

/// Compute the nominal/primitive type category of an evaluated value.
pub fn type_of_value(v: &L3ValueV2) -> L3ValueType {
    match v {
        L3ValueV2::Int(_) => L3ValueType::Int,
        L3ValueV2::Str(_) => L3ValueType::Str,
        L3ValueV2::Bool(_) => L3ValueType::Bool,
        L3ValueV2::Ctor { nominal_sum, .. } => L3ValueType::Sum(nominal_sum.clone()),
        L3ValueV2::Record { nominal_config, .. } => L3ValueType::Record(nominal_config.clone()),
    }
}

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

/// The complete report of a finite-decision deliberation run.
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
}

impl FiniteDecisionRun {
    /// Whether a candidate was selected.
    pub fn is_selected(&self) -> bool {
        matches!(self.stop, FiniteDecisionStop::Selected(_))
    }

    /// Whether deliberation halted in certified quiescence.
    pub fn is_quiescent(&self) -> bool {
        matches!(self.stop, FiniteDecisionStop::Quiescent { .. })
    }

    /// Whether deliberation halted with an Unknown fault.
    pub fn is_unknown(&self) -> bool {
        matches!(self.stop, FiniteDecisionStop::Unknown(_))
    }

    /// Retrieve the structured status disposition of candidate `name`.
    pub fn status_of(&self, name: &str) -> Option<CandidateStatus> {
        self.dispositions
            .iter()
            .find(|d| d.name == name)
            .map(|d| d.status.clone())
    }

    /// Look up a bound input record by name.
    pub fn input(&self, name: &str) -> Option<&BoundInput> {
        self.inputs.iter().find(|i| i.name == name)
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
    interner: Interner,
    initial: Handle,
    policy_handle: Handle,
    entries: Vec<PresenterEntry>,
    plan: FiniteDecisionPlan,
    snapshot: InputSnapshot,
    bound_inputs: Vec<BoundInput>,
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
        let mut entries = Vec::with_capacity(plan.commit.candidates.len());
        for cand_name in &plan.commit.candidates {
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

        Ok(Self {
            program,
            context,
            initial_world,
            policy,
            interner,
            initial,
            policy_handle,
            entries,
            plan: plan.clone(),
            snapshot: snapshot.clone(),
            bound_inputs,
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
        // Step 0: Inject bound inputs into evaluation environment before lets/rules/proposals.
        let mut env = EvalEnv::new();
        for input in &self.bound_inputs {
            env = env.with_input(input.name.clone(), input.value.clone());
        }

        // Step 1: Evaluate closed let bindings.
        for (name, expr) in &self.plan.lets {
            match eval(expr, &env) {
                Ok(v) => env = env.with_let(name.clone(), v),
                Err(fault) => {
                    return FiniteDecisionRun {
                        program: self.program,
                        context: self.context,
                        inputs: self.bound_inputs.clone(),
                        facts: Vec::new(),
                        decision: None,
                        dispositions: Vec::new(),
                        journal: Journal::new(),
                        final_world: self.initial_world,
                        stop: FiniteDecisionStop::Unknown(
                            FiniteDecisionUnknownReason::ExpressionEvaluationFault {
                                context: format!("let {name}"),
                                fault,
                            },
                        ),
                    };
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
                    return FiniteDecisionRun {
                        program: self.program,
                        context: self.context,
                        inputs: self.bound_inputs.clone(),
                        facts,
                        decision: None,
                        dispositions: Vec::new(),
                        journal: Journal::new(),
                        final_world: self.initial_world,
                        stop: FiniteDecisionStop::Unknown(
                            FiniteDecisionUnknownReason::ExpressionEvaluationFault {
                                context: format!("rule {}", rule.name),
                                fault,
                            },
                        ),
                    };
                }
            }
        }

        // Step 3: Evaluate proposal guards and values in the commit pool.
        let mut admitted_names = BTreeSet::new();
        let mut candidate_values = Vec::new();

        for cand_name in &self.plan.commit.candidates {
            let Some(proposal) = self.plan.find_proposal(cand_name) else {
                return FiniteDecisionRun {
                    program: self.program,
                    context: self.context,
                    inputs: self.bound_inputs.clone(),
                    facts,
                    decision: None,
                    dispositions: Vec::new(),
                    journal: Journal::new(),
                    final_world: self.initial_world,
                    stop: FiniteDecisionStop::Unknown(
                        FiniteDecisionUnknownReason::DependencyFault {
                            context: format!("candidate proposal {cand_name}"),
                            detail: "proposal missing from plan".to_string(),
                        },
                    ),
                };
            };

            // Evaluate guard: must be Bool.
            match eval(&proposal.guard, &env) {
                Ok(L3ValueV2::Bool(true)) => {
                    admitted_names.insert(cand_name.clone());
                }
                Ok(L3ValueV2::Bool(false)) => {}
                Ok(other) => {
                    return FiniteDecisionRun {
                        program: self.program,
                        context: self.context,
                        inputs: self.bound_inputs.clone(),
                        facts,
                        decision: None,
                        dispositions: Vec::new(),
                        journal: Journal::new(),
                        final_world: self.initial_world,
                        stop: FiniteDecisionStop::Unknown(FiniteDecisionUnknownReason::TypeFault {
                            context: format!("proposal {} guard", proposal.name),
                            detail: format!(
                                "guard must evaluate to Bool, found {}",
                                type_of_value(&other)
                            ),
                        }),
                    };
                }
                Err(fault) => {
                    return FiniteDecisionRun {
                        program: self.program,
                        context: self.context,
                        inputs: self.bound_inputs.clone(),
                        facts,
                        decision: None,
                        dispositions: Vec::new(),
                        journal: Journal::new(),
                        final_world: self.initial_world,
                        stop: FiniteDecisionStop::Unknown(
                            FiniteDecisionUnknownReason::ExpressionEvaluationFault {
                                context: format!("proposal {} guard", proposal.name),
                                fault,
                            },
                        ),
                    };
                }
            }

            // Evaluate value.
            match eval(&proposal.value, &env) {
                Ok(v) => candidate_values.push((cand_name.clone(), v)),
                Err(fault) => {
                    return FiniteDecisionRun {
                        program: self.program,
                        context: self.context,
                        inputs: self.bound_inputs.clone(),
                        facts,
                        decision: None,
                        dispositions: Vec::new(),
                        journal: Journal::new(),
                        final_world: self.initial_world,
                        stop: FiniteDecisionStop::Unknown(
                            FiniteDecisionUnknownReason::ExpressionEvaluationFault {
                                context: format!("proposal {} value", proposal.name),
                                fault,
                            },
                        ),
                    };
                }
            }
        }

        // Step 4: All proposal values must share one type.
        if candidate_values.len() > 1 {
            let expected_type = type_of_value(&candidate_values[0].1);
            for (name, val) in &candidate_values[1..] {
                let actual_type = type_of_value(val);
                if actual_type != expected_type {
                    return FiniteDecisionRun {
                        program: self.program,
                        context: self.context,
                        inputs: self.bound_inputs.clone(),
                        facts,
                        decision: None,
                        dispositions: Vec::new(),
                        journal: Journal::new(),
                        final_world: self.initial_world,
                        stop: FiniteDecisionStop::Unknown(FiniteDecisionUnknownReason::TypeFault {
                            context: "proposal values".to_string(),
                            detail: format!(
                                "proposal '{name}' value type {actual_type} does not match expected {expected_type}"
                            ),
                        }),
                    };
                }
            }
        }

        // Step 5: Build NamedCandidates and evaluate the deliberation frontier.
        let regime_id = RegimeId::named(FINITE_FRONTIER_REGIME_NAME);
        let named_candidates: Vec<NamedCandidate> = self
            .entries
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
            &self.interner,
        );

        // Fail-closed on frontier deliberation fault under B^uk discipline.
        if let Some(fault) = evaluated.fault() {
            return FiniteDecisionRun {
                program: self.program,
                context: self.context,
                inputs: self.bound_inputs.clone(),
                facts,
                decision: None,
                dispositions: Vec::new(),
                journal: Journal::new(),
                final_world: self.initial_world,
                stop: FiniteDecisionStop::Unknown(FiniteDecisionUnknownReason::from(fault)),
            };
        }

        // Compute structured dispositions for all candidates in the commit pool.
        let mut dispositions = Vec::new();
        for cand_name in &self.plan.commit.candidates {
            let Some(nc) = named_candidates.iter().find(|c| &c.name == cand_name) else {
                return FiniteDecisionRun {
                    program: self.program,
                    context: self.context,
                    inputs: self.bound_inputs.clone(),
                    facts,
                    decision: None,
                    dispositions: Vec::new(),
                    journal: Journal::new(),
                    final_world: self.initial_world,
                    stop: FiniteDecisionStop::Unknown(
                        FiniteDecisionUnknownReason::EvaluationError {
                            detail: format!(
                                "candidate '{cand_name}' missing from named candidates"
                            ),
                        },
                    ),
                };
            };
            let Some(status) = evaluated.status_of(nc) else {
                return FiniteDecisionRun {
                    program: self.program,
                    context: self.context,
                    inputs: self.bound_inputs.clone(),
                    facts,
                    decision: None,
                    dispositions: Vec::new(),
                    journal: Journal::new(),
                    final_world: self.initial_world,
                    stop: FiniteDecisionStop::Unknown(
                        FiniteDecisionUnknownReason::EvaluationError {
                            detail: format!("candidate status missing for '{cand_name}'"),
                        },
                    ),
                };
            };
            dispositions.push(CandidateDisposition {
                name: cand_name.clone(),
                priority: nc.priority,
                status,
            });
        }

        // Step 6: Selection or Certified Quiescence.
        if evaluated.is_quiescent() {
            // All candidates rejected is successful certified quiescence with decision None.
            let all_presenter = CandidatePresenter {
                initial: self.initial,
                entries: self.entries.clone(),
            };
            let adm_adapter = PolicyToAdmAdapter::new(&policy, named_candidates.iter().cloned());
            let all_generators: BTreeSet<GeneratorId> =
                self.entries.iter().map(|e| e.generator).collect();
            let obs_profile = match GeneratorPartitionProfile::new(all_generators, BTreeSet::new())
            {
                Ok(p) => p,
                Err(err) => {
                    return FiniteDecisionRun {
                        program: self.program,
                        context: self.context,
                        inputs: self.bound_inputs.clone(),
                        facts,
                        decision: None,
                        dispositions,
                        journal: Journal::new(),
                        final_world: self.initial_world,
                        stop: FiniteDecisionStop::Unknown(
                            FiniteDecisionUnknownReason::QuiescenceVerificationFault {
                                detail: format!("invalid observation profile: {err:?}"),
                            },
                        ),
                    };
                }
            };
            let pres = PresentationV1 {
                id: PresentationIdV1::from_canon(self.program.digest().as_bytes()),
                regimes: &[&all_presenter],
                regime_set: Digest::of(Domain::Value, b"brix.l3.finite-decision.regime-set@1"),
                adm: &adm_adapter,
                adm_id: Digest::of(Domain::Value, b"brix.l3.finite-decision.adm@1"),
                profile: &obs_profile,
                interner: &self.interner,
                context: self.context,
                assumptions: DeclaredAssumptions::all(),
            };
            let mut k = |c: &Candidate, phase: u64| {
                if let Some(entry) = self.entries.iter().find(|e| e.candidate == *c) {
                    Key::new(phase, entry.priority, entry.tiebreak)
                } else {
                    Key::new(
                        phase,
                        u64::MAX,
                        Digest::of(Domain::Value, b"missing-candidate"),
                    )
                }
            };
            let (step, _, _) = sat_step(&pres, &exec, 0, &mut k, SaturationBudget::uniform(32));
            let certificate = match step {
                SaturatedStep::Quiescent(cert) => {
                    let cert_id = quiescence_certificate_id(&cert);
                    let check = check_quiescence_certificate(&cert, &pres, &exec, &[]);
                    if !matches!(check, CertificateCheck::Verified { .. }) {
                        return FiniteDecisionRun {
                            program: self.program,
                            context: self.context,
                            inputs: self.bound_inputs.clone(),
                            facts,
                            decision: None,
                            dispositions,
                            journal: Journal::new(),
                            final_world: self.initial_world,
                            stop: FiniteDecisionStop::Unknown(
                                FiniteDecisionUnknownReason::QuiescenceVerificationFault {
                                    detail: format!(
                                        "quiescence certificate verification failed: {check:?}"
                                    ),
                                },
                            ),
                        };
                    }
                    cert_id
                }
                other => {
                    return FiniteDecisionRun {
                        program: self.program,
                        context: self.context,
                        inputs: self.bound_inputs.clone(),
                        facts,
                        decision: None,
                        dispositions,
                        journal: Journal::new(),
                        final_world: self.initial_world,
                        stop: FiniteDecisionStop::Unknown(
                            FiniteDecisionUnknownReason::QuiescenceVerificationFault {
                                detail: format!(
                                    "saturation did not return quiescence certificate: {other:?}"
                                ),
                            },
                        ),
                    };
                }
            };

            FiniteDecisionRun {
                program: self.program,
                context: self.context,
                inputs: self.bound_inputs.clone(),
                facts,
                decision: None,
                dispositions,
                journal: Journal::new(),
                final_world: self.initial_world,
                stop: FiniteDecisionStop::Quiescent { certificate },
            }
        } else {
            // Exactly one candidate was selected.
            let Some((_, winning_cand)) = evaluated.selected.as_ref() else {
                return FiniteDecisionRun {
                    program: self.program,
                    context: self.context,
                    inputs: self.bound_inputs.clone(),
                    facts,
                    decision: None,
                    dispositions,
                    journal: Journal::new(),
                    final_world: self.initial_world,
                    stop: FiniteDecisionStop::Unknown(
                        FiniteDecisionUnknownReason::EvaluationError {
                            detail: "expected selected candidate in evaluated frontier".to_string(),
                        },
                    ),
                };
            };
            let admitted_entries: Vec<PresenterEntry> = self
                .entries
                .iter()
                .filter(|e| admitted_names.contains(&e.name))
                .cloned()
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
                &self.interner,
                &self.initial_exec(),
                self.context,
                0,
                &mut keyer,
            );

            let (committed, step, _) = match tick_res {
                Ok(triple) => triple,
                Err(err) => {
                    return FiniteDecisionRun {
                        program: self.program,
                        context: self.context,
                        inputs: self.bound_inputs.clone(),
                        facts,
                        decision: None,
                        dispositions,
                        journal: Journal::new(),
                        final_world: self.initial_world,
                        stop: FiniteDecisionStop::Unknown(
                            FiniteDecisionUnknownReason::CommitTickError {
                                detail: format!("{err:?}"),
                            },
                        ),
                    };
                }
            };

            let Committed::Step { observation, .. } = committed else {
                return FiniteDecisionRun {
                    program: self.program,
                    context: self.context,
                    inputs: self.bound_inputs.clone(),
                    facts,
                    decision: None,
                    dispositions,
                    journal: Journal::new(),
                    final_world: self.initial_world,
                    stop: FiniteDecisionStop::Unknown(
                        FiniteDecisionUnknownReason::CommitTickError {
                            detail: "expected committed step".to_string(),
                        },
                    ),
                };
            };

            // Contract: Results are Derived, never Proven or Refuted.
            if observation.outcome_class != Outcome::Derived {
                return FiniteDecisionRun {
                    program: self.program,
                    context: self.context,
                    inputs: self.bound_inputs.clone(),
                    facts,
                    decision: None,
                    dispositions,
                    journal: Journal::new(),
                    final_world: self.initial_world,
                    stop: FiniteDecisionStop::Unknown(
                        FiniteDecisionUnknownReason::CommitTickError {
                            detail: format!(
                                "committed observation grade {:?} is not Derived",
                                observation.outcome_class
                            ),
                        },
                    ),
                };
            }

            let Some(step) = step else {
                return FiniteDecisionRun {
                    program: self.program,
                    context: self.context,
                    inputs: self.bound_inputs.clone(),
                    facts,
                    decision: None,
                    dispositions,
                    journal: Journal::new(),
                    final_world: self.initial_world,
                    stop: FiniteDecisionStop::Unknown(
                        FiniteDecisionUnknownReason::CommitTickError {
                            detail: "Committed::Step missing journal record".to_string(),
                        },
                    ),
                };
            };
            let mut journal = Journal::new();
            journal.append(step);

            let winning_val = match candidate_values
                .into_iter()
                .find(|(n, _)| n == &winning_cand.name)
            {
                Some((_, val)) => val,
                None => {
                    return FiniteDecisionRun {
                        program: self.program,
                        context: self.context,
                        inputs: self.bound_inputs.clone(),
                        facts,
                        decision: None,
                        dispositions,
                        journal: Journal::new(),
                        final_world: self.initial_world,
                        stop: FiniteDecisionStop::Unknown(
                            FiniteDecisionUnknownReason::EvaluationError {
                                detail: format!(
                                    "winning candidate '{}' value missing from evaluated candidate values",
                                    winning_cand.name
                                ),
                            },
                        ),
                    };
                }
            };

            let decision = SelectedDecision {
                candidate: winning_cand.name.clone(),
                priority: winning_cand.priority,
                value: winning_val,
                grade: Outcome::Derived,
            };

            FiniteDecisionRun {
                program: self.program,
                context: self.context,
                inputs: self.bound_inputs.clone(),
                facts,
                decision: Some(decision.clone()),
                dispositions,
                journal,
                final_world: winning_cand.dst,
                stop: FiniteDecisionStop::Selected(decision),
            }
        }
    }

    /// Re-derive why a candidate was admitted/selected fresh from inputs.
    pub fn explain_why(
        &self,
        target_name: &str,
    ) -> Result<WhyExplanation, FiniteDecisionUnknownReason> {
        let run = self.run();
        if let FiniteDecisionStop::Unknown(reason) = run.stop {
            return Err(reason);
        }

        let regime_id = RegimeId::named(FINITE_FRONTIER_REGIME_NAME);
        let named_candidates: Vec<NamedCandidate> = self
            .entries
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

        let Some(target) = named_candidates.iter().find(|c| c.name == target_name) else {
            return Ok(WhyExplanation::CandidateNotFound);
        };

        let admitted_names: BTreeSet<String> = run
            .dispositions
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
        let explanation = explain_why(
            &named_candidates,
            &policy,
            &self.initial_exec(),
            target,
            &self.interner,
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
        if let FiniteDecisionStop::Unknown(reason) = run.stop {
            return Err(reason);
        }

        let regime_id = RegimeId::named(FINITE_FRONTIER_REGIME_NAME);
        let named_candidates: Vec<NamedCandidate> = self
            .entries
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

        let Some(target) = named_candidates.iter().find(|c| c.name == target_name) else {
            return Ok(WhyNotExplanation::CandidateNotFound);
        };

        let admitted_names: BTreeSet<String> = run
            .dispositions
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
        let explanation = explain_why_not(
            &named_candidates,
            &policy,
            &self.initial_exec(),
            target,
            &self.interner,
        );
        if let WhyNotExplanation::EvaluationFaulted { fault, .. } = &explanation {
            return Err(FiniteDecisionUnknownReason::from(fault.clone()));
        }
        Ok(explanation)
    }
    /// Re-evaluate all declared show expressions against the runtime's bound inputs, lets, and derived facts.
    pub fn evaluate_shows(
        &self,
        run: &FiniteDecisionRun,
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

        let mut env = EvalEnv::new();
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
        let mut results = Vec::with_capacity(self.plan.shows.len());
        for (idx, show_expr) in self.plan.shows.iter().enumerate() {
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
    for cand_name in &plan.commit.candidates {
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
