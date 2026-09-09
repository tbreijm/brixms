//! Source-derived audit input bundle verification and production (ADR-0026).

use std::fmt;

use brix_canon::Digest;
use brix_semantic::ContextId;
use brix_syntax::ParseLimits;
use soc_core::audit_bundle::{
    check_audit_input_bundle_v1, produce_audit_input_bundle_with_limits_v1, AuditDecodeLimits,
    BundleCheckError, BundleProducerError, SettlementAuditInputBundleIdV1,
    SettlementAuditInputBundleV1,
};
use soc_core::audit_receipt::SettlementAuditReceiptIdV1;

use crate::finite_decision::plan::{
    finite_decision_program_id, FiniteDecisionProgramId, FINITE_DECISION_PROFILE,
};
use crate::finite_decision::runtime::{
    FiniteDecisionRun, FiniteDecisionRuntime, FiniteDecisionStop,
};
use crate::l3::{lower_l3_plan, L3PlanV1, PlanLimitsV1, L3_PROFILE_MARKER_V1};
use crate::l3_audit::{l3_generator_registry, l3_generator_semantics};
use crate::l3_canon::{context_id, policy_id, program_id, ProgramIdV1, RunContextV1};
use crate::l3_regime::{build_l3_transition_table, l3_policy};
use crate::l3_run::{L3RunReport, SettlementRunV1, SettlementStopV1};

/// Verification report for an audit input bundle.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AuditBundleVerificationReport<P> {
    /// The program identity.
    pub program: P,
    /// The run context identity.
    pub context: ContextId,
    /// The audit input bundle identity.
    pub bundle_id: SettlementAuditInputBundleIdV1,
    /// The final chain digest.
    pub final_chain: Digest,
    /// The verified receipt identities in commit order.
    pub receipt_ids: Vec<SettlementAuditReceiptIdV1>,
    /// Number of verified steps.
    pub count: usize,
}

impl<P> AuditBundleVerificationReport<P> {
    /// Return the verification status string.
    pub fn status(&self) -> &'static str {
        "audit-bundle-verified"
    }
}

impl<P: fmt::Debug> fmt::Display for AuditBundleVerificationReport<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "program: {:?}\ncontext: {}\nbundle_id: {}\nfinal_chain: {}\nreceipts: {}\nstatus: {}",
            self.program,
            self.context.digest().to_hex(),
            self.bundle_id.digest().to_hex(),
            self.final_chain.to_hex(),
            self.count,
            self.status()
        )
    }
}

/// A verification report for a preserved L3 v1 audit input bundle.
pub type L3AuditBundleVerificationReport = AuditBundleVerificationReport<ProgramIdV1>;

/// A verification report for a finite-decision audit input bundle.
pub type FiniteDecisionAuditBundleVerificationReport =
    AuditBundleVerificationReport<FiniteDecisionProgramId>;

/// Error conditions when verifying an audit input bundle against source.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SourceBundleError<P> {
    /// The source exceeds configured byte limits.
    SourceTooLarge {
        /// Configured limit in bytes.
        limit: usize,
        /// Observed length in bytes.
        found: usize,
    },
    /// The source slice is not valid UTF-8.
    InvalidUtf8,
    /// Parsing failed or exceeded parser limits.
    Parse(String),
    /// Lowering failed or exceeded limits.
    Lower(String),
    /// The derived program does not match the external expected target.
    ProgramMismatch {
        /// Expected program identity.
        expected: P,
        /// Derived program identity.
        derived: P,
    },
    /// The bundle context does not match the derived context.
    ContextMismatch {
        /// Expected context identity.
        expected: ContextId,
        /// Observed bundle context identity.
        found: ContextId,
    },
    /// Input validation failed against declared inputs.
    InputValidation(crate::input::InputValidationError),
    /// Bundle validation or receipt verification failed.
    BundleCheck(BundleCheckError),
}

impl<P: fmt::Debug> fmt::Display for SourceBundleError<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceTooLarge { limit, found } => {
                write!(
                    f,
                    "source exceeds maximum size: limit {limit}, found {found}"
                )
            }
            Self::InvalidUtf8 => write!(f, "source is not valid UTF-8"),
            Self::Parse(err) => write!(f, "source parse failed: {err}"),
            Self::Lower(err) => write!(f, "source lowering failed: {err}"),
            Self::ProgramMismatch { expected, derived } => {
                write!(
                    f,
                    "program identity mismatch: expected {expected:?}, derived {derived:?}"
                )
            }
            Self::ContextMismatch { expected, found } => {
                write!(
                    f,
                    "context identity mismatch: expected {expected:?}, found {found:?}"
                )
            }
            Self::InputValidation(err) => write!(f, "input validation failed: {err}"),
            Self::BundleCheck(err) => write!(f, "bundle verification failed: {err:?}"),
        }
    }
}

impl<P: fmt::Debug> std::error::Error for SourceBundleError<P> {}

/// Error type for L3 v1 bundle verification.
pub type L3SourceBundleError = SourceBundleError<ProgramIdV1>;

/// Error type for finite-decision bundle verification.
pub type FiniteDecisionSourceBundleError = SourceBundleError<FiniteDecisionProgramId>;

/// Error conditions when producing an audit input bundle.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SourceBundleProducerError {
    /// The run resulted in an Unknown outcome.
    UnknownRun,
    /// The runtime and run parameters disagree.
    RunMismatch(String),
    /// Error during bundle production.
    BundleProducer(BundleProducerError),
}

impl fmt::Display for SourceBundleProducerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownRun => write!(f, "refused: run outcome is Unknown"),
            Self::RunMismatch(msg) => write!(f, "refused: runtime and run mismatch: {msg}"),
            Self::BundleProducer(err) => write!(f, "bundle production failed: {err:?}"),
        }
    }
}

impl std::error::Error for SourceBundleProducerError {}

impl From<BundleProducerError> for SourceBundleProducerError {
    fn from(err: BundleProducerError) -> Self {
        Self::BundleProducer(err)
    }
}

/// Derive the run context, generator registry, and semantics for an L3 v1 plan.
pub fn l3_audit_environment(
    plan: &L3PlanV1,
) -> (
    ContextId,
    brix_semantic::GeneratorRegistry,
    soc_core::audit::GeneratorSemanticsV1,
) {
    let mut interner = soc_core::intern::Interner::new();
    let table = build_l3_transition_table(&mut interner, plan);
    let program = program_id(plan);
    let compiled_policy = l3_policy(program, &table);
    let policy_config = policy_id(&compiled_policy);
    let run_context = RunContextV1 {
        program,
        initial_world: table.initial_world_config(),
        policy: policy_config,
        profile: L3_PROFILE_MARKER_V1.to_string(),
        limits: plan.limits,
    };
    let context = context_id(&run_context);
    let registry = l3_generator_registry(&table);
    let semantics = l3_generator_semantics(&table);
    (context, registry, semantics)
}

/// Verify a preserved L3 v1 audit input bundle against an already parsed and resolved module.
pub fn check_l3_audit_input_bundle_from_module_v1(
    module: &brix_syntax::ast::Module,
    expected_program: ProgramIdV1,
    plan_limits: &PlanLimitsV1,
    bundle: &SettlementAuditInputBundleV1,
    decode_limits: &AuditDecodeLimits,
) -> Result<L3AuditBundleVerificationReport, L3SourceBundleError> {
    let plan = lower_l3_plan(module, L3_PROFILE_MARKER_V1, plan_limits)
        .map_err(|e| SourceBundleError::Lower(format!("{e:?}")))?;

    let derived_program = program_id(&plan);
    if derived_program != expected_program {
        return Err(SourceBundleError::ProgramMismatch {
            expected: expected_program,
            derived: derived_program,
        });
    }

    let (context, registry, semantics) = l3_audit_environment(&plan);

    if bundle.context != context {
        return Err(SourceBundleError::ContextMismatch {
            expected: context,
            found: bundle.context,
        });
    }

    let receipt_ids = check_audit_input_bundle_v1(bundle, &registry, &semantics, decode_limits)
        .map_err(SourceBundleError::BundleCheck)?;

    let count = receipt_ids.len();
    Ok(AuditBundleVerificationReport {
        program: expected_program,
        context,
        bundle_id: bundle.id(),
        final_chain: bundle.final_chain_digest,
        receipt_ids,
        count,
    })
}

/// Verify a preserved L3 v1 audit input bundle against source.
pub fn check_l3_audit_input_bundle_from_source_v1(
    source: &[u8],
    expected_program: ProgramIdV1,
    parse_limits: ParseLimits,
    plan_limits: &PlanLimitsV1,
    bundle: &SettlementAuditInputBundleV1,
    decode_limits: &AuditDecodeLimits,
) -> Result<L3AuditBundleVerificationReport, L3SourceBundleError> {
    if source.len() > parse_limits.max_source_bytes {
        return Err(SourceBundleError::SourceTooLarge {
            limit: parse_limits.max_source_bytes,
            found: source.len(),
        });
    }

    let text = std::str::from_utf8(source).map_err(|_| SourceBundleError::InvalidUtf8)?;

    let module = brix_syntax::parse_bounded(text, parse_limits)
        .map_err(|e| SourceBundleError::Parse(e.to_string()))?;

    check_l3_audit_input_bundle_from_module_v1(
        &module,
        expected_program,
        plan_limits,
        bundle,
        decode_limits,
    )
}

/// Produce an audit input bundle from an L3 v1 run report and run under explicit decode limits.
pub fn produce_l3_audit_input_bundle_with_limits_v1(
    report: &L3RunReport,
    run: &SettlementRunV1,
    limits: &AuditDecodeLimits,
) -> Result<SettlementAuditInputBundleV1, SourceBundleProducerError> {
    match &run.stop {
        SettlementStopV1::Unknown { .. } => return Err(SourceBundleProducerError::UnknownRun),
        SettlementStopV1::Quiescent { .. } => {}
    }
    match &report.run.stop {
        SettlementStopV1::Unknown { .. } => return Err(SourceBundleProducerError::UnknownRun),
        SettlementStopV1::Quiescent { .. } => {}
    }

    if run.program != report.run.program {
        return Err(SourceBundleProducerError::RunMismatch(
            "program mismatch between report and run".to_string(),
        ));
    }
    if run.context != report.context {
        return Err(SourceBundleProducerError::RunMismatch(
            "context mismatch between report and run".to_string(),
        ));
    }
    if run.chain_digest != report.journal.chain_digest() {
        return Err(SourceBundleProducerError::RunMismatch(
            "chain digest mismatch between report and run".to_string(),
        ));
    }
    if run.step_digests != report.run.step_digests {
        return Err(SourceBundleProducerError::RunMismatch(
            "step digests mismatch between report and run".to_string(),
        ));
    }
    if *run != report.run {
        return Err(SourceBundleProducerError::RunMismatch(
            "report run does not match provided run".to_string(),
        ));
    }

    let registry = l3_generator_registry(&report.table);
    let semantics = l3_generator_semantics(&report.table);

    produce_audit_input_bundle_with_limits_v1(
        &report.journal,
        report.context,
        &registry,
        &semantics,
        limits,
    )
    .map_err(SourceBundleProducerError::BundleProducer)
}

/// Produce an audit input bundle from an L3 v1 run report and run under strict decode limits.
pub fn produce_l3_audit_input_bundle_v1(
    report: &L3RunReport,
    run: &SettlementRunV1,
) -> Result<SettlementAuditInputBundleV1, SourceBundleProducerError> {
    produce_l3_audit_input_bundle_with_limits_v1(report, run, &AuditDecodeLimits::strict())
}

/// Verify a finite-decision audit input bundle against an already parsed and resolved module under an input snapshot.
pub fn check_finite_decision_audit_input_bundle_from_module_with_inputs_v1(
    module: &brix_syntax::ast::Module,
    expected_program: FiniteDecisionProgramId,
    plan_limits: &PlanLimitsV1,
    bundle: &SettlementAuditInputBundleV1,
    decode_limits: &AuditDecodeLimits,
    snapshot: &crate::input::InputSnapshot,
) -> Result<FiniteDecisionAuditBundleVerificationReport, FiniteDecisionSourceBundleError> {
    let plan = crate::finite_decision::lower_finite_decision_plan(module, FINITE_DECISION_PROFILE)
        .map_err(|e| SourceBundleError::Lower(format!("{e:?}")))?;

    if (plan.rules.len() as u64) > plan_limits.max_selected_rules {
        return Err(SourceBundleError::Lower(format!(
            "selected rules limit exceeded: {} > {}",
            plan.rules.len(),
            plan_limits.max_selected_rules
        )));
    }

    let derived_program = finite_decision_program_id(&plan);
    if derived_program != expected_program {
        return Err(SourceBundleError::ProgramMismatch {
            expected: expected_program,
            derived: derived_program,
        });
    }

    let (context, registry, semantics) =
        crate::finite_decision::runtime::finite_decision_audit_environment_from_plan_with_inputs(
            &plan, snapshot,
        )
        .map_err(|e| match e {
            crate::finite_decision::runtime::FiniteDecisionBuildError::InputValidation(err) => {
                SourceBundleError::InputValidation(err)
            }
            crate::finite_decision::runtime::FiniteDecisionBuildError::MissingProposal {
                candidate,
            } => SourceBundleError::Lower(format!(
                "candidate '{candidate}' in commit was not found in declared proposals"
            )),
        })?;

    if bundle.context != context {
        return Err(SourceBundleError::ContextMismatch {
            expected: context,
            found: bundle.context,
        });
    }

    let receipt_ids = check_audit_input_bundle_v1(bundle, &registry, &semantics, decode_limits)
        .map_err(SourceBundleError::BundleCheck)?;

    let count = receipt_ids.len();
    Ok(AuditBundleVerificationReport {
        program: expected_program,
        context,
        bundle_id: bundle.id(),
        final_chain: bundle.final_chain_digest,
        receipt_ids,
        count,
    })
}

/// Verify a finite-decision audit input bundle against an already parsed and resolved module with no external inputs.
pub fn check_finite_decision_audit_input_bundle_from_module_v1(
    module: &brix_syntax::ast::Module,
    expected_program: FiniteDecisionProgramId,
    plan_limits: &PlanLimitsV1,
    bundle: &SettlementAuditInputBundleV1,
    decode_limits: &AuditDecodeLimits,
) -> Result<FiniteDecisionAuditBundleVerificationReport, FiniteDecisionSourceBundleError> {
    check_finite_decision_audit_input_bundle_from_module_with_inputs_v1(
        module,
        expected_program,
        plan_limits,
        bundle,
        decode_limits,
        &crate::input::InputSnapshot::empty(),
    )
}

/// Verify a finite-decision audit input bundle against source under an input snapshot.
pub fn check_finite_decision_audit_input_bundle_from_source_with_inputs_v1(
    source: &[u8],
    expected_program: FiniteDecisionProgramId,
    parse_limits: ParseLimits,
    plan_limits: &PlanLimitsV1,
    bundle: &SettlementAuditInputBundleV1,
    decode_limits: &AuditDecodeLimits,
    snapshot: &crate::input::InputSnapshot,
) -> Result<FiniteDecisionAuditBundleVerificationReport, FiniteDecisionSourceBundleError> {
    if source.len() > parse_limits.max_source_bytes {
        return Err(SourceBundleError::SourceTooLarge {
            limit: parse_limits.max_source_bytes,
            found: source.len(),
        });
    }

    let text = std::str::from_utf8(source).map_err(|_| SourceBundleError::InvalidUtf8)?;

    let module = brix_syntax::parse_bounded(text, parse_limits)
        .map_err(|e| SourceBundleError::Parse(e.to_string()))?;

    check_finite_decision_audit_input_bundle_from_module_with_inputs_v1(
        &module,
        expected_program,
        plan_limits,
        bundle,
        decode_limits,
        snapshot,
    )
}

/// Verify a finite-decision audit input bundle against source with no external inputs.
pub fn check_finite_decision_audit_input_bundle_from_source_v1(
    source: &[u8],
    expected_program: FiniteDecisionProgramId,
    parse_limits: ParseLimits,
    plan_limits: &PlanLimitsV1,
    bundle: &SettlementAuditInputBundleV1,
    decode_limits: &AuditDecodeLimits,
) -> Result<FiniteDecisionAuditBundleVerificationReport, FiniteDecisionSourceBundleError> {
    check_finite_decision_audit_input_bundle_from_source_with_inputs_v1(
        source,
        expected_program,
        parse_limits,
        plan_limits,
        bundle,
        decode_limits,
        &crate::input::InputSnapshot::empty(),
    )
}

/// Produce an audit input bundle from a finite-decision runtime and run under explicit decode limits.
pub fn produce_finite_decision_audit_input_bundle_with_limits_v1(
    runtime: &FiniteDecisionRuntime,
    run: &FiniteDecisionRun,
    limits: &AuditDecodeLimits,
) -> Result<SettlementAuditInputBundleV1, SourceBundleProducerError> {
    if run.is_unknown() {
        return Err(SourceBundleProducerError::UnknownRun);
    }
    match &run.stop {
        FiniteDecisionStop::Unknown(_) => return Err(SourceBundleProducerError::UnknownRun),
        FiniteDecisionStop::Selected(_) | FiniteDecisionStop::Quiescent { .. } => {}
    }

    if run.program != runtime.program {
        return Err(SourceBundleProducerError::RunMismatch(
            "program mismatch between runtime and run".to_string(),
        ));
    }

    if run.context != runtime.context {
        return Err(SourceBundleProducerError::RunMismatch(
            "context mismatch between runtime and run".to_string(),
        ));
    }

    if run.inputs != runtime.bound_inputs() {
        return Err(SourceBundleProducerError::RunMismatch(
            "bound inputs mismatch between runtime and run".to_string(),
        ));
    }

    if !run.journal.is_empty() {
        let first_step = &run.journal.steps()[0];
        if first_step.src != runtime.initial_world {
            return Err(SourceBundleProducerError::RunMismatch(
                "initial world mismatch between runtime and journal".to_string(),
            ));
        }
        let last_step = run.journal.steps().last().unwrap();
        if last_step.dst != run.final_world {
            return Err(SourceBundleProducerError::RunMismatch(
                "final world mismatch between journal and run".to_string(),
            ));
        }
    } else if run.final_world != runtime.initial_world {
        return Err(SourceBundleProducerError::RunMismatch(
            "quiescent run final world mismatch".to_string(),
        ));
    }

    let (context, registry, semantics) = runtime.audit_environment();

    produce_audit_input_bundle_with_limits_v1(&run.journal, context, &registry, &semantics, limits)
        .map_err(SourceBundleProducerError::BundleProducer)
}

/// Produce an audit input bundle from a finite-decision runtime and run under strict decode limits.
pub fn produce_finite_decision_audit_input_bundle_v1(
    runtime: &FiniteDecisionRuntime,
    run: &FiniteDecisionRun,
) -> Result<SettlementAuditInputBundleV1, SourceBundleProducerError> {
    produce_finite_decision_audit_input_bundle_with_limits_v1(
        runtime,
        run,
        &AuditDecodeLimits::strict(),
    )
}
