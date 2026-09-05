pub mod audit;
pub mod check;
pub mod run;
pub mod verify;
pub mod why;

use brix_lower::finite_decision::runtime::{
    CandidateDisposition, DerivedFact, FiniteDecisionRun, FiniteDecisionStop, SelectedDecision,
};
use brix_lower::l3_v2::L3ValueV2;
use soc_regimes::finite_frontier::CandidateStatus;

use crate::json::{to_tagged_value, CandidateJson, DecisionJson, FactJson, StructuredReasonJson};

/// Prepare an AST module for finite-decision lowering by removing surface `show` directives.
pub fn prepare_finite_decision_module(module: &mut brix_syntax::ast::Module) {
    module
        .items
        .retain(|i| !matches!(i, brix_syntax::ast::Item::Show(_)));
}

/// Format an [`L3ValueV2`] for human-readable display.
pub fn fmt_value_human(v: &L3ValueV2) -> String {
    match v {
        L3ValueV2::Int(n) => n.to_string(),
        L3ValueV2::Bool(b) => b.to_string(),
        L3ValueV2::Str(s) => format!("\"{s}\""),
        L3ValueV2::Ctor { variant, args, .. } => {
            if args.is_empty() {
                variant.clone()
            } else {
                let inner = args
                    .iter()
                    .map(fmt_value_human)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{variant}({inner})")
            }
        }
        L3ValueV2::Record {
            nominal_config,
            fields,
        } => {
            let inner = fields
                .iter()
                .map(|(k, val)| format!("{k}: {}", fmt_value_human(val)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{nominal_config} {{ {inner} }}")
        }
    }
}

/// Convert a [`DerivedFact`] into its canonical JSON representation.
pub fn fact_to_json(f: &DerivedFact) -> FactJson {
    FactJson {
        name: f.rule.clone(),
        value: to_tagged_value(&f.value),
        ordinal: f.ordinal.to_string(),
        grade: "Derived".to_string(),
    }
}

/// Convert a [`SelectedDecision`] into its canonical JSON representation.
pub fn decision_to_json(d: &SelectedDecision) -> DecisionJson {
    DecisionJson {
        candidate: d.candidate.clone(),
        priority: d.priority.to_string(),
        value: to_tagged_value(&d.value),
        grade: "Derived".to_string(),
    }
}

use brix_lower::finite_decision::FiniteDecisionUnknownReason;

/// Map a deliberation unknown reason to a stable reason code and human detail string.
pub fn unknown_reason_to_code_and_detail(
    reason: &FiniteDecisionUnknownReason,
) -> (&'static str, String) {
    let code = match reason {
        FiniteDecisionUnknownReason::ExpressionEvaluationFault { .. } => {
            "expression-evaluation-fault"
        }
        FiniteDecisionUnknownReason::DependencyFault { .. } => "dependency-fault",
        FiniteDecisionUnknownReason::TypeFault { .. } => "type-fault",
        FiniteDecisionUnknownReason::DecisionKeyConflict { .. } => "decision-key-conflict",
        FiniteDecisionUnknownReason::AdmissionError { .. } => "admission-error",
        FiniteDecisionUnknownReason::EvaluationError { .. } => "evaluation-error",
        FiniteDecisionUnknownReason::InvalidPhase { .. } => "invalid-phase",
        FiniteDecisionUnknownReason::QuiescenceVerificationFault { .. } => {
            "quiescence-verification-fault"
        }
        FiniteDecisionUnknownReason::CommitTickError { .. } => "commit-tick-error",
        FiniteDecisionUnknownReason::InvariantViolation { .. } => "invariant-violation",
    };
    (code, reason.to_string())
}

/// Convert a candidate disposition and winning candidate context into [`CandidateJson`].
pub fn candidate_disposition_to_json(
    d: &CandidateDisposition,
    winning_cand: Option<&str>,
) -> CandidateJson {
    let (status, code, detail) = match &d.status {
        CandidateStatus::Selected => (
            "selected".to_string(),
            "selected".to_string(),
            "selected: minimal calendar key".to_string(),
        ),
        CandidateStatus::AdmittedNotSelected => {
            let win = winning_cand.unwrap_or("another candidate");
            (
                "admitted-not-selected".to_string(),
                "overshadowed".to_string(),
                format!("admitted but overshadowed by candidate '{win}'"),
            )
        }
        CandidateStatus::RejectedGuardFalse => (
            "rejected-guard-false".to_string(),
            "guard_false@1".to_string(),
            "guard condition evaluated to false".to_string(),
        ),
        CandidateStatus::Rejected(r) => (
            "rejected".to_string(),
            r.category().to_string(),
            format!("{r}"),
        ),
    };

    CandidateJson {
        name: d.name.clone(),
        priority: d.priority.to_string(),
        status,
        reason: StructuredReasonJson { code, detail },
    }
}

/// Format human output for finite-decision deliberation.
///
/// Disciplinary rule: Never prints Proven or Refuted.
/// Leads with facts, candidate dispositions, decision or quiescence, and reasons; IDs follow.
pub fn format_finite_decision_human(run: &FiniteDecisionRun, context_hex: Option<&str>) -> String {
    let mut out = String::new();

    // 1. Facts
    if !run.facts.is_empty() {
        out.push_str("facts:\n");
        for f in &run.facts {
            out.push_str(&format!(
                "  {}: {} @Derived\n",
                f.rule,
                fmt_value_human(&f.value)
            ));
        }
    }

    // 2. Candidate dispositions
    if !run.dispositions.is_empty() {
        let winning_name = run.decision.as_ref().map(|d| d.candidate.as_str());
        out.push_str("candidates:\n");
        for d in &run.dispositions {
            let candidate_json = candidate_disposition_to_json(d, winning_name);
            out.push_str(&format!(
                "  {}: {} (priority {}) — {}\n",
                d.name, candidate_json.status, d.priority, candidate_json.reason.detail
            ));
        }
    }

    // 3. Decision or Quiescence
    match &run.stop {
        FiniteDecisionStop::Selected(sel) => {
            out.push_str(&format!(
                "decision: {} = {} @Derived\nstatus: selected\n",
                sel.candidate,
                fmt_value_human(&sel.value)
            ));
        }
        FiniteDecisionStop::Quiescent { .. } => {
            out.push_str("decision: none (quiescent)\nstatus: quiescent\n");
        }
        FiniteDecisionStop::Unknown(reason) => {
            out.push_str(&format!("status: unknown ({reason})\n"));
        }
    }

    // 4. IDs follow
    out.push_str(&format!("program: {}\n", run.program.0.to_hex()));
    if let Some(ctx) = context_hex {
        out.push_str(&format!("context: {}\n", ctx));
    }
    if let FiniteDecisionStop::Quiescent { certificate } = &run.stop {
        out.push_str(&format!("certificate: {}\n", certificate.digest().to_hex()));
    }

    out
}
