//! Finite-decision alpha profile (`brix.l3.finite-decision@1`) — ADR-0030.
//!
//! Reuses L3 v2 expression lowering, evaluation, and types alongside
//! the `soc-regimes` finite deliberation frontier.

pub mod plan;
pub mod runtime;

pub use plan::{
    finite_decision_program_id, finite_decision_program_preimage, lower_finite_decision_plan,
    FiniteDecisionCommit, FiniteDecisionLowerError, FiniteDecisionPlan, FiniteDecisionProgramId,
    FiniteDecisionProposal, FiniteDecisionRule, FINITE_DECISION_PROFILE,
};
pub use runtime::{
    finite_decision_audit_environment_from_plan, run_finite_decision_plan, type_of_value,
    CandidateDisposition, DerivedFact, FiniteDecisionRun, FiniteDecisionRuntime,
    FiniteDecisionStop, FiniteDecisionUnknownReason, L3ValueType, SelectedDecision,
};
pub use soc_core::saturate::QuiescenceCertificateId;
pub use soc_regimes::finite_frontier::{CandidateStatus, WhyExplanation, WhyNotExplanation};

pub use crate::audit_bundle::{
    check_finite_decision_audit_input_bundle_from_module_v1,
    check_finite_decision_audit_input_bundle_from_source_v1,
    produce_finite_decision_audit_input_bundle_v1,
    produce_finite_decision_audit_input_bundle_with_limits_v1,
    FiniteDecisionAuditBundleVerificationReport, FiniteDecisionSourceBundleError,
};
