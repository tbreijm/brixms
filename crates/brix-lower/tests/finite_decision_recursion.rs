//! Recursion in the finite-decision lane (ADR-0042, superseding ADR-0032's
//! refusal of direct and mutual recursion among `fn` helpers).
//!
//! Coverage: a terminating happy path, mutual recursion, recursion near and
//! past the call-depth bound, budget exhaustion distinct from the call-depth
//! bound, determinism, and audit/verify replay.

use brix_canon::Canonical;
use brix_lower::l3_v2::{EvalFault, L3ValueV2, MAX_CALL_DEPTH};
use brix_lower::{
    check_finite_decision_audit_input_bundle_from_source_v1, decode_audit_input_bundle_v1,
    finite_decision_program_id, lower_finite_decision_plan,
    produce_finite_decision_audit_input_bundle_v1, AuditDecodeLimits, FiniteDecisionPlan,
    FiniteDecisionRuntime, FiniteDecisionStop, FiniteDecisionUnknownReason, PlanLimitsV1,
    FINITE_DECISION_PROFILE,
};
use brix_syntax::{parse, ParseLimits};

fn plan(source: &str) -> FiniteDecisionPlan {
    let module = parse(source).expect("fixture parses");
    lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).expect("fixture lowers")
}

fn expression_fault(run: &brix_lower::FiniteDecisionRun) -> &EvalFault {
    match &run.stop {
        FiniteDecisionStop::Unknown(FiniteDecisionUnknownReason::ExpressionEvaluationFault {
            fault,
            ..
        }) => fault,
        other => panic!("expected an expression evaluation fault, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Happy path: direct and mutual recursion that terminates.
// ---------------------------------------------------------------------------

#[test]
fn test_direct_recursion_happy_path_sums_to_n() {
    let source = r#"
config Decision = Done

fn sum_to(n: Int): Int = match n == 0 {
  true => 0
  false => n + sum_to(n - 1)
}

rule total() = sum_to(10)
propose p(total) priority 10 when total == 55 = Done
commit c from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected(), "expected a selected decision, got {:?}", run.stop);
    assert_eq!(run.facts[0].value, L3ValueV2::Int(55));
}

#[test]
fn test_mutual_recursion_happy_path_is_even() {
    let source = r#"
config Decision = Done

fn is_even(n: Int): Bool = match n == 0 {
  true => true
  false => is_odd(n - 1)
}
fn is_odd(n: Int): Bool = match n == 0 {
  true => false
  false => is_even(n - 1)
}

rule parity() = is_even(40)
propose p(parity) priority 10 when parity = Done
commit c from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());
    assert_eq!(run.facts[0].value, L3ValueV2::Bool(true));
}

// ---------------------------------------------------------------------------
// Near and past the call-depth bound.
// ---------------------------------------------------------------------------

fn countdown_source(depth: usize) -> String {
    format!(
        r#"
config Decision = Done

fn countdown(x: Int): Int = match x == 0 {{
  true => 0
  false => countdown(x - 1)
}}

rule r() = countdown({depth})
propose p(r) priority 10 when true = Done
commit c from (p)
"#
    )
}

#[test]
fn test_recursion_just_under_call_depth_bound_succeeds() {
    // Comfortably under the bound, but far past what the old (64) bound
    // would have admitted — this is meant to actually exercise deep
    // recursion, not merely avoid the limit.
    let depth = MAX_CALL_DEPTH - 10;
    let p = plan(&countdown_source(depth));
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(
        run.is_selected(),
        "recursion to depth {depth} (bound {MAX_CALL_DEPTH}) must succeed, got {:?}",
        run.stop
    );
    assert_eq!(run.facts[0].value, L3ValueV2::Int(0));
}

#[test]
fn test_recursion_past_call_depth_bound_is_unknown_not_a_crash() {
    let depth = MAX_CALL_DEPTH + 500;
    let p = plan(&countdown_source(depth));
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_unknown());
    assert!(matches!(
        expression_fault(&run),
        EvalFault::CallDepthExceeded { .. }
    ));
}

// ---------------------------------------------------------------------------
// Budget exhaustion distinct from the call-depth bound: a recursive function
// whose *work per level* grows (exponential fan-out), so the step/value
// budget is exhausted long before `MAX_CALL_DEPTH` is ever reached.
// ---------------------------------------------------------------------------

#[test]
fn test_recursion_resource_budget_exhaustion_is_unknown() {
    let source = r#"
config Tree = Leaf | Node(Tree, Tree)
config Decision = Done

fn build(n: Int) = match n == 0 {
  true => Leaf
  false => Node(build(n - 1), build(n - 1))
}

rule r() = build(40)
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_unknown());
    assert!(matches!(
        expression_fault(&run),
        EvalFault::ResourceExhausted { .. }
    ));
}

// ---------------------------------------------------------------------------
// Determinism.
// ---------------------------------------------------------------------------

#[test]
fn test_recursion_is_deterministic_across_runs() {
    let source = countdown_source(200);
    let p = plan(&source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run_a = runtime.run();
    let run_b = runtime.run();
    assert_eq!(run_a.facts, run_b.facts);
    assert_eq!(run_a.decision, run_b.decision);
}

// ---------------------------------------------------------------------------
// Audit / verify replay.
// ---------------------------------------------------------------------------

#[test]
fn test_recursive_helper_survives_audit_bundle_replay() {
    let source = r#"
config Decision = Approved | Rejected

fn factorial(n: Int): Int = match n == 0 {
  true => 1
  false => n * factorial(n - 1)
}

rule score() = factorial(6)
propose approve(score) priority 10 when score == 720 = Approved
propose reject() priority 100 when true = Rejected
commit c from (approve, reject)
"#;
    let p = plan(source);
    let prog_id = finite_decision_program_id(&p);

    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());
    assert_eq!(run.decision.as_ref().unwrap().candidate, "approve");

    let bundle = produce_finite_decision_audit_input_bundle_v1(&runtime, &run)
        .expect("bundle produces cleanly");
    let bundle_bytes = bundle.canon_bytes();
    let decoded = decode_audit_input_bundle_v1(&bundle_bytes, &AuditDecodeLimits::strict())
        .expect("bundle decodes cleanly");

    let report = check_finite_decision_audit_input_bundle_from_source_v1(
        source.as_bytes(),
        prog_id,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &decoded,
        &AuditDecodeLimits::strict(),
    )
    .expect("recursive-helper replay verifies");

    assert_eq!(report.program, prog_id);
    assert_eq!(report.context, run.context);
    assert_eq!(report.status(), "audit-bundle-verified");
}
