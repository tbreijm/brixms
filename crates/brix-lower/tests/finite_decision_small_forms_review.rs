//! Regression tests for Checkpoint 3 review findings.
//!
//! 1. List annotations must not silently accept mismatched reference types.
//! 2. Wildcard arm must not match a value belonging to a different nominal type.
//! 3. Explanation must share arm-selection logic with evaluation (bare vs qualified).
//! 4. Unreachable wildcard arm diagnostic must apply to simple programs without helpers or schemas.

use brix_lower::finite_decision::{
    lower_finite_decision_plan, FiniteDecisionRuntime, FiniteDecisionStop,
    FiniteDecisionUnknownReason, FINITE_DECISION_PROFILE,
};
use brix_lower::l3_v2::EvalFault;
use brix_syntax::parse;

fn try_plan(
    source: &str,
) -> Result<brix_lower::FiniteDecisionPlan, brix_lower::FiniteDecisionLowerError> {
    let module = parse(source).expect("fixture parses");
    lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
}

fn refusal(source: &str) -> String {
    format!(
        "{:?}",
        try_plan(source).expect_err("fixture must be refused")
    )
}

// ---------------------------------------------------------------------------
// 1. List annotations with reference types
// ---------------------------------------------------------------------------

#[test]
fn list_annotation_rejects_reference_element_type_mismatch() {
    let source = r#"
config Decision = Ship
let n = 7
rule r() = len([n]: List<Str>)
propose p(r) priority 10 when r == 1 = Ship
commit c from (p)
"#;
    assert!(refusal(source).contains("AscriptionMismatch"));
}

#[test]
fn list_annotation_rejects_non_list_reference() {
    let source = r#"
config Decision = Ship
let n = 7
rule r() = len(n: List<Str>)
propose p(r) priority 10 when r == 1 = Ship
commit c from (p)
"#;
    assert!(refusal(source).contains("AscriptionMismatch"));
}

// ---------------------------------------------------------------------------
// 2. Wildcard arm across nominal boundaries
// ---------------------------------------------------------------------------

#[test]
fn wildcard_arm_rejects_value_from_different_nominal_type() {
    let source = r#"
config Documentary = Spec | Draft
config Runtime = Executed | Running
config Decision = Ship | Hold
let e = Executed
rule r() = match e { Spec => false  _ => true }
propose p(r) priority 10 when r = Ship
commit c from (p)
"#;
    let plan = try_plan(source).expect("plan lowers");
    let runtime = FiniteDecisionRuntime::build(&plan).expect("runtime builds");
    let run = runtime.run();
    assert!(
        !run.is_selected(),
        "wrong nominal type must not match wildcard arm"
    );
    match run.stop {
        FiniteDecisionStop::Unknown(FiniteDecisionUnknownReason::ExpressionEvaluationFault {
            fault,
            ..
        }) => {
            assert!(matches!(fault, EvalFault::OperandShape(_)));
        }
        other => panic!("expected operand shape fault, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 3. Explanation arm selection for qualified/unqualified constructors
// ---------------------------------------------------------------------------

#[test]
fn explanation_reports_correct_arm_not_wildcard_for_qualified_match() {
    use brix_lower::finite_decision::{ExplainOutcome, TraceOutcome};
    let source = r#"
config Q = Volume | Trend
config Decision = Ship | Hold
let q = Q::Volume
rule r() = match q { Q::Volume => 1  _ => 9 }
propose p(r) priority 10 when r == 1 = Ship
commit c from (p)
"#;
    let plan = try_plan(source).expect("plan lowers");
    let runtime = FiniteDecisionRuntime::build(&plan).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());
    let ExplainOutcome::Explained(expl) = runtime.explain_candidate("p").expect("explain succeeds")
    else {
        panic!("candidate p not explained");
    };
    let fact_r = expl
        .facts
        .iter()
        .find(|f| f.name == "r")
        .expect("fact r found");
    let trace = fact_r.trace.as_ref().expect("trace present");
    assert_eq!(trace.children[1].source, "Q::Volume => 1");
    assert!(matches!(trace.children[1].outcome, TraceOutcome::Value(_)));
    assert_eq!(trace.children[2].source, "_ => 9");
    assert_eq!(trace.children[2].outcome, TraceOutcome::NotEvaluated);
}

// ---------------------------------------------------------------------------
// 4. Unreachable wildcard in simple programs
// ---------------------------------------------------------------------------

#[test]
fn unreachable_wildcard_is_refused_in_simple_program() {
    let source = r#"
config Q = Volume | Trend | Rank | Share
config Decision = Ship | Hold
let q = Trend
rule r() = match q { Volume => 1  Trend => 2  Rank => 3  Share => 4  _ => 9 }
propose p(r) priority 10 when true = Ship
commit c from (p)
"#;
    assert!(refusal(source).contains("UnreachableWildcardArm"));
}
