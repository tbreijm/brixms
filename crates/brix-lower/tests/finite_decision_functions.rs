//! Regression and integration tests for finite-decision functions (ADR-0032).

use brix_canon::Canonical;
use brix_lower::l3_v2::{EvalFault, L3ValueV2};
use brix_lower::{
    canonicalize_input_shards, check_finite_decision_audit_input_bundle_from_source_v1,
    check_finite_decision_audit_input_bundle_from_source_with_inputs_v1,
    decode_audit_input_bundle_v1, decode_input_shard, finite_decision_program_id,
    finite_decision_program_preimage, lower_finite_decision_plan,
    produce_finite_decision_audit_input_bundle_v1, AuditDecodeLimits, FiniteDecisionLowerError,
    FiniteDecisionPlan, FiniteDecisionRuntime, FiniteDecisionStop, FiniteDecisionUnknownReason,
    InputLimits, PlanLimitsV1, FINITE_DECISION_PROFILE, MAX_EXPR_DEPTH, MAX_EXPR_NODES,
    MAX_FUNCTION_COUNT, MAX_FUNCTION_PARAMS,
};
use brix_syntax::{parse, ParseLimits};
use soc_regimes::finite_frontier::{WhyExplanation, WhyNotExplanation};

fn plan(source: &str) -> FiniteDecisionPlan {
    let module = parse(source).expect("fixture parses");
    lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).expect("fixture lowers")
}

fn snapshot_from_json(json: &str) -> brix_lower::InputSnapshot {
    let limits = InputLimits::default();
    let shard = decode_input_shard(json.as_bytes(), &limits).expect("shard decodes");
    canonicalize_input_shards(vec![shard], &limits).expect("snapshot canonicalizes")
}

// ---------------------------------------------------------------------------
// 1. Call sites: let, rule, propose guard, propose value, show
// ---------------------------------------------------------------------------

#[test]
fn test_call_in_let_binding() {
    let source = r#"
config Decision = Expedite | Ship | Hold

fn add_bonus(x: Int, bonus: Int): Int = x + bonus

let base = add_bonus(10, 5)

rule r() = base

propose p(r) priority 10 when r == 15 = Ship
commit c from (p)
show base
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());
    let dec = run.decision.as_ref().unwrap();
    assert_eq!(dec.candidate, "p");

    let shows = runtime.evaluate_shows(&run).expect("shows evaluate");
    assert_eq!(shows, vec![L3ValueV2::Int(15)]);
}

#[test]
fn test_call_in_rule_body() {
    let source = r#"
config Decision = Expedite | Ship | Hold

fn double(x: Int): Int = x + x

rule r1() = 10
rule r2(r1) = double(r1)

propose p(r2) priority 10 when r2 == 20 = Ship
commit c from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());
    assert_eq!(run.facts.len(), 2);
    assert_eq!(run.facts[1].rule, "r2");
    assert_eq!(run.facts[1].value, L3ValueV2::Int(20));
}

#[test]
fn test_call_in_proposal_guard_and_value() {
    let source = r#"
config Decision = Win(Int) | Lose

fn is_even(n: Int): Bool = match n == 0 {
  true => true
  false => match n == 2 {
    true => true
    false => match n == 4 {
      true => true
      false => false
    }
  }
}

fn score(base: Int, mult: Int): Int = base * mult

rule x() = 4

propose p(x) priority 10 when is_even(x) = Win(score(x, 10))
propose fallback() priority 100 when true = Lose
commit c from (p, fallback)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());
    let dec = run.decision.unwrap();
    assert_eq!(dec.candidate, "p");
    assert_eq!(
        dec.value,
        L3ValueV2::Ctor {
            nominal_sum: "Decision".to_string(),
            variant: "Win".to_string(),
            args: vec![L3ValueV2::Int(40)],
        }
    );
}

#[test]
fn test_call_in_show_directive() {
    let source = r#"
config Decision = Done

fn triple(n: Int): Int = n * 3

rule r() = 7

propose p() priority 10 when true = Done
commit c from (p)
show triple(r)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    let shows = runtime.evaluate_shows(&run).expect("shows evaluate");
    assert_eq!(shows, vec![L3ValueV2::Int(21)]);
}

// ---------------------------------------------------------------------------
// 2. Nested Helper Calls
// ---------------------------------------------------------------------------

#[test]
fn test_nested_helper_calls() {
    let source = r#"
config Decision = Done

fn h(x: Int): Int = x + 1
fn g(x: Int): Int = h(x) * 2
fn f(x: Int): Int = g(x) + 3

rule res() = f(5)

propose p(res) priority 10 when res == 15 = Done
commit c from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());
    assert_eq!(run.facts[0].value, L3ValueV2::Int(15));
}

// ---------------------------------------------------------------------------
// 3. Shadowing and Lexical Scoping
// ---------------------------------------------------------------------------

#[test]
fn test_parameter_shadows_global_let_name() {
    // Parameter `x` inside `f` has value 10, not 999.
    let source = r#"
config Decision = Done

let x = 999

fn f(x: Int): Int = x + 1

rule r() = f(10)

propose p(r) priority 10 when r == 11 = Done
commit c from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());
    assert_eq!(run.facts[0].value, L3ValueV2::Int(11));
}

#[test]
fn test_match_binder_shadows_parameter() {
    let source = r#"
config Box = Val(Int)
config Decision = Done

fn unwrap_or_add(b, x: Int): Int = match b {
  Val(x) => x + 10
}

rule r() = unwrap_or_add(Val(5), 100)

propose p(r) priority 10 when r == 15 = Done
commit c from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());
    assert_eq!(run.facts[0].value, L3ValueV2::Int(15));
}

// ---------------------------------------------------------------------------
// 4. Closed Helper Semantics: Refusal of Hidden Reads
// ---------------------------------------------------------------------------

#[test]
fn test_closed_helper_rejects_reading_rule_fact() {
    let source = r#"
config Decision = Done

rule r() = 42

fn bad_helper(x: Int): Int = x + r

propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::RuleFactReadInFunction { func, fact } if func == "bad_helper" && fact == "r"
    ));
}

#[test]
fn test_closed_helper_rejects_reading_input() {
    let source = r#"
config Decision = Done

input stock: Int

fn bad_helper(x: Int): Int = x + stock

propose p() priority 10 when true = Done
commit c from (p)
"#;
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::InputReadInFunction { func, input } if func == "bad_helper" && input == "stock"
    ));
}

#[test]
fn test_closed_helper_rejects_reading_global_let() {
    let source = r#"
config Decision = Done

let global_val = 100

fn bad_helper(x: Int): Int = x + global_val

propose p() priority 10 when true = Done
commit c from (p)
"#;
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::GlobalLetReadInFunction { func, binding } if func == "bad_helper" && binding == "global_val"
    ));
}

// ---------------------------------------------------------------------------
// 5. Eager Call-By-Value & Unused-Argument Faults
// ---------------------------------------------------------------------------

#[test]
fn test_eager_unused_argument_arithmetic_overflow_fails_closed() {
    // `const_first` drops `unused`, but `unused` causes i64 overflow.
    // The fault must NOT be optimized away!
    let source = r#"
config Decision = Done

fn const_first(a: Int, unused: Int): Int = a

rule r() = const_first(42, 9223372036854775807 + 1)

propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_unknown());
    match run.stop {
        FiniteDecisionStop::Unknown(FiniteDecisionUnknownReason::ExpressionEvaluationFault {
            context,
            fault,
        }) => {
            assert!(context.contains("rule r"));
            assert!(matches!(fault, EvalFault::Overflow(_)));
        }
        other => panic!("expected arithmetic overflow fault, found {other:?}"),
    }
}

#[test]
fn test_eager_unused_argument_contract_violation_fails_closed() {
    // `const_first` takes `unused: Bool`, but receives `100`.
    // The contract violation in the unused argument must cause failure!
    let source = r#"
config Decision = Done

fn const_first(a: Int, unused: Bool): Int = a

rule r() = const_first(42, 100)

propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_unknown());
    match run.stop {
        FiniteDecisionStop::Unknown(FiniteDecisionUnknownReason::ExpressionEvaluationFault {
            context,
            fault,
        }) => {
            assert!(context.contains("rule r"));
            assert!(matches!(fault, EvalFault::ContractViolation { .. }));
        }
        other => panic!("expected contract violation fault, found {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 6. Contracts: Types, Nominal Configs, and Grades
// ---------------------------------------------------------------------------

#[test]
fn test_function_contract_argument_type_mismatch() {
    let source = r#"
config Decision = Done

fn check_int(x: Int): Int = x + 1

rule r() = check_int(true)

propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_unknown());
    match run.stop {
        FiniteDecisionStop::Unknown(FiniteDecisionUnknownReason::ExpressionEvaluationFault {
            context,
            fault,
        }) => {
            assert!(context.contains("rule r"));
            assert!(matches!(
                fault,
                EvalFault::ContractViolation {
                    param: Some(ref p),
                    expected,
                    found,
                    ..
                } if p == "x" && expected == "Int" && found == "Bool"
            ));
        }
        other => panic!("expected parameter contract violation, found {other:?}"),
    }
}

#[test]
fn test_function_contract_return_type_mismatch() {
    let source = r#"
config Decision = Done

fn bad_return(x: Int): Bool = x + 1

rule r() = bad_return(10)

propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_unknown());
    match run.stop {
        FiniteDecisionStop::Unknown(FiniteDecisionUnknownReason::ExpressionEvaluationFault {
            context,
            fault,
        }) => {
            assert!(context.contains("rule r"));
            assert!(matches!(
                fault,
                EvalFault::ContractViolation {
                    param: None,
                    expected,
                    found,
                    ..
                } if expected == "Bool" && found == "Int"
            ));
        }
        other => panic!("expected return contract violation, found {other:?}"),
    }
}

#[test]
fn test_unannotated_sum_and_record_arguments() {
    let source = r#"
config Status = Active | Inactive
config User = { name: Str, age: Int }
config Decision = Allow | Deny

fn is_active(s): Bool = match s {
  Active => true
  Inactive => false
}

fn user_name(u): Str = u.name

rule s() = is_active(Active)
rule u() = user_name(User { name: "Alice", age: 30 })

propose p(s, u) priority 10 when s == true = Allow
commit c from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());
    assert_eq!(run.facts[0].value, L3ValueV2::Bool(true));
    assert_eq!(run.facts[1].value, L3ValueV2::Str("Alice".to_string()));
}

#[test]
fn test_reject_nominal_composite_contracts() {
    let src1 = r#"
config Status = Active | Inactive
config Decision = Done

fn check_status(s: Status): Bool = true

rule r() = 1
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let module1 = parse(src1).expect("parses");
    let err1 = lower_finite_decision_plan(&module1, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err1,
        FiniteDecisionLowerError::UnsupportedContractType { ref ty, .. } if ty == "Status"
    ));

    let src2 = r#"
config User = { name: Str, age: Int }
config Decision = Done

fn check_user(u: User): Str = "ok"

rule r() = 1
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let module2 = parse(src2).expect("parses");
    let err2 = lower_finite_decision_plan(&module2, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err2,
        FiniteDecisionLowerError::UnsupportedContractType { ref ty, .. } if ty == "User"
    ));
}

#[test]
fn test_reject_unsupported_contract_grade_proven() {
    let source = r#"
config Decision = Done

fn f(x: Int @Proven): Int = x

rule r() = f(1)
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::UnsupportedContractGrade { .. }
    ));
}

#[test]
fn test_reject_unsupported_contract_grade_audited() {
    let source = r#"
config Decision = Done

fn f(x: Int): Int @Audited = x

rule r() = f(1)
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::UnsupportedContractGrade { .. }
    ));
}

#[test]
fn test_accept_derived_grade() {
    let source = r#"
config Decision = Done

fn f(x: Int @Derived): Int @Derived = x + 1

rule r() = f(10)
propose p(r) priority 10 when r == 11 = Done
commit c from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());
    assert_eq!(run.facts[0].value, L3ValueV2::Int(11));
}

#[test]
fn test_reject_unknown_contract_type() {
    let source = r#"
config Decision = Done

fn f(x: NonExistentType): Int = 1

rule r() = f(1)
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::UnknownContractType { .. }
    ));
}

// ---------------------------------------------------------------------------
// 7. Cycle Detection
// ---------------------------------------------------------------------------

#[test]
fn test_direct_recursion_rejected() {
    let source = r#"
config Decision = Done

fn self_loop(x: Int): Int = self_loop(x - 1)

rule r() = self_loop(10)
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::FunctionCycle { ref func, ref cycle } if func == "self_loop" && cycle.contains(&"self_loop".to_string())
    ));
}

#[test]
fn test_mutual_recursion_rejected() {
    let source = r#"
config Decision = Done

fn ping(x: Int): Int = pong(x)
fn pong(x: Int): Int = ping(x)

rule r() = ping(10)
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::FunctionCycle { .. }
    ));
}

#[test]
fn test_three_hop_cycle_rejected() {
    let source = r#"
config Decision = Done

fn a(x: Int): Int = b(x)
fn b(x: Int): Int = c(x)
fn c(x: Int): Int = a(x)

rule r() = a(10)
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::FunctionCycle { .. }
    ));
}

// ---------------------------------------------------------------------------
// 8. Bounds & Limits
// ---------------------------------------------------------------------------

#[test]
fn test_duplicate_function_name() {
    let source = r#"
config Decision = Done

fn my_fn(x: Int): Int = x
fn my_fn(x: Int): Int = x + 1

rule r() = my_fn(1)
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::DuplicateFunctionName(ref name) if name == "my_fn"
    ));
}

#[test]
fn test_duplicate_parameter_name() {
    let source = r#"
config Decision = Done

fn duplicate_params(x: Int, x: Int): Int = x

rule r() = duplicate_params(1, 2)
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::DuplicateFunctionParameter { ref func, ref param } if func == "duplicate_params" && param == "x"
    ));
}

#[test]
fn test_function_arity_mismatch() {
    let source = r#"
config Decision = Done

fn f(x: Int, y: Int): Int = x + y

rule r() = f(1)
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::FunctionArityMismatch { ref func, expected: 2, found: 1 } if func == "f"
    ));
}

#[test]
fn test_too_many_functions_limit() {
    let mut source = String::from("config Decision = Done\n");
    for i in 0..=MAX_FUNCTION_COUNT {
        source.push_str(&format!("fn f{i}(): Int = {i}\n"));
    }
    source.push_str(
        "rule r() = f0()\npropose p(r) priority 10 when true = Done\ncommit c from (p)\n",
    );
    let module = parse(&source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::TooManyFunctions { limit, count } if limit == MAX_FUNCTION_COUNT && count == MAX_FUNCTION_COUNT + 1
    ));
}

#[test]
fn test_too_many_function_params_limit() {
    let mut params = Vec::new();
    for i in 0..=MAX_FUNCTION_PARAMS {
        params.push(format!("p{i}: Int"));
    }
    let source = format!(
        "config Decision = Done\nfn big_fn({}): Int = p0\nrule r() = 1\npropose p(r) priority 10 when true = Done\ncommit c from (p)\n",
        params.join(", ")
    );
    let module = parse(&source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::TooManyFunctionParams { limit, count, .. } if limit == MAX_FUNCTION_PARAMS && count == MAX_FUNCTION_PARAMS + 1
    ));
}

#[test]
fn test_expression_depth_limit() {
    // Build an expression nested deeper than MAX_EXPR_DEPTH (128).
    let mut expr = "1".to_string();
    for _ in 0..=MAX_EXPR_DEPTH {
        expr = format!("({expr} + 1)");
    }
    let source = format!(
        "config Decision = Done\nfn deep_fn(): Int = {expr}\nrule r() = 1\npropose p(r) priority 10 when true = Done\ncommit c from (p)\n"
    );
    let module = parse(&source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::ExpressionDepthExceeded { limit, .. } if limit == MAX_EXPR_DEPTH
    ));
}

#[test]
fn test_expression_node_limit() {
    // Build a balanced tree with depth 12: 2^13 - 1 = 8191 nodes, depth 12 <= 128.
    fn build_balanced(d: usize) -> String {
        if d == 0 {
            "1".to_string()
        } else {
            format!("({} + {})", build_balanced(d - 1), build_balanced(d - 1))
        }
    }
    let expr = build_balanced(12);
    let source = format!(
        "config Decision = Done\nfn big_expr(): Int = {expr}\nrule r() = 1\npropose p(r) priority 10 when true = Done\ncommit c from (p)\n"
    );
    let module = parse(&source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::ExpressionNodeLimitExceeded { limit, .. } if limit == MAX_EXPR_NODES
    ));
}

// ---------------------------------------------------------------------------
// 9. Canonical Program Identity & Backward Compatibility
// ---------------------------------------------------------------------------

#[test]
fn test_program_id_deterministic_and_comment_invariant() {
    let src1 = r#"
config Decision = Done

// Helper function
fn add(a: Int, b: Int): Int = a + b

rule r() = add(3, 4)

propose p(r) priority 10 when r == 7 = Done
commit c from (p)
"#;

    let src2 = r#"
config Decision = Done
fn add(a: Int, b: Int): Int = a + b
rule r() = add(3, 4)
propose p(r) priority 10 when r == 7 = Done
commit c from (p)
"#;

    let p1 = plan(src1);
    let p2 = plan(src2);
    assert_eq!(
        finite_decision_program_id(&p1),
        finite_decision_program_id(&p2)
    );
    assert_eq!(
        finite_decision_program_preimage(&p1),
        finite_decision_program_preimage(&p2)
    );
}

#[test]
fn test_program_id_sensitive_to_function_body_change() {
    let src1 = r#"
config Decision = Done
fn add(a: Int, b: Int): Int = a + b
rule r() = add(3, 4)
propose p(r) priority 10 when r == 7 = Done
commit c from (p)
"#;

    let src2 = r#"
config Decision = Done
fn add(a: Int, b: Int): Int = a + b + 1
rule r() = add(3, 4)
propose p(r) priority 10 when r == 7 = Done
commit c from (p)
"#;

    let p1 = plan(src1);
    let p2 = plan(src2);
    assert_ne!(
        finite_decision_program_id(&p1),
        finite_decision_program_id(&p2)
    );
}

#[test]
fn test_zero_function_preimage_byte_identical_compatibility() {
    // When no functions are declared, the preimage must not emit the functions tag.
    let src = r#"
config Decision = A | B
rule r() = 1
propose p(r) priority 10 when r == 1 = A
commit c from (p)
"#;
    let p = plan(src);
    assert!(p.functions.is_empty());
    let preimage = finite_decision_program_preimage(&p);
    // Preimage must not contain the functions frame tag
    assert!(!preimage
        .windows(b"brix.l3.finite-decision.functions@1".len())
        .any(|window| window == b"brix.l3.finite-decision.functions@1"));
}

// ---------------------------------------------------------------------------
// 10. Audit Bundle Production, Independent Verification, and Tamper Detection
// ---------------------------------------------------------------------------

#[test]
fn test_audit_bundle_production_and_cross_verification_with_functions() {
    let source = r#"
config Decision = Approved | Rejected

fn calc_risk(score: Int, penalty: Int): Int = score - penalty
fn is_approved(risk: Int): Bool = risk >= 50

rule score() = 80
rule penalty() = 15
rule net_risk(score, penalty) = calc_risk(score, penalty)

propose approve(net_risk) priority 10 when is_approved(net_risk) = Approved
propose reject() priority 100 when true = Rejected
commit c from (approve, reject)
"#;
    let p = plan(source);
    let prog_id = finite_decision_program_id(&p);

    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());
    assert_eq!(run.decision.as_ref().unwrap().candidate, "approve");

    // Produce audit bundle
    let bundle = produce_finite_decision_audit_input_bundle_v1(&runtime, &run)
        .expect("bundle produces cleanly");

    let bundle_bytes = bundle.canon_bytes();
    let decoded = decode_audit_input_bundle_v1(&bundle_bytes, &AuditDecodeLimits::strict())
        .expect("bundle decodes cleanly");

    // Verify bundle against source
    let report = check_finite_decision_audit_input_bundle_from_source_v1(
        source.as_bytes(),
        prog_id,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &decoded,
        &AuditDecodeLimits::strict(),
    )
    .expect("verification succeeds");

    assert_eq!(report.program, prog_id);
    assert_eq!(report.context, run.context);
    assert_eq!(report.status(), "audit-bundle-verified");
}

#[test]
fn test_audit_bundle_tamper_detection_in_function() {
    let source = r#"
config Decision = Approved | Rejected
fn calc(x: Int): Int = x + 1
rule r() = calc(10)
propose p(r) priority 10 when r == 11 = Approved
commit c from (p)
"#;
    let p = plan(source);
    let prog_id = finite_decision_program_id(&p);

    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    let bundle = produce_finite_decision_audit_input_bundle_v1(&runtime, &run)
        .expect("bundle produces cleanly");

    let bundle_bytes = bundle.canon_bytes();
    let decoded = decode_audit_input_bundle_v1(&bundle_bytes, &AuditDecodeLimits::strict())
        .expect("bundle decodes cleanly");

    // Tamper the function in source: `x + 2` instead of `x + 1`.
    let tampered_source = r#"
config Decision = Approved | Rejected
fn calc(x: Int): Int = x + 2
rule r() = calc(10)
propose p(r) priority 10 when r == 11 = Approved
commit c from (p)
"#;

    let result = check_finite_decision_audit_input_bundle_from_source_v1(
        tampered_source.as_bytes(),
        prog_id,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &decoded,
        &AuditDecodeLimits::strict(),
    );

    // Must fail due to program mismatch
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// 11. Certified Quiescence & Explanation Support
// ---------------------------------------------------------------------------

#[test]
fn test_quiescence_with_functions_and_explanations() {
    let source = r#"
config Decision = OptionA | OptionB

fn is_even(n: Int): Bool = match n == 2 {
  true => true
  false => false
}

rule n() = 3

propose opt_a(n) priority 10 when is_even(n) = OptionA
commit c from (opt_a)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_quiescent());

    // Check why: opt_a was not selected
    let why = runtime.explain_why("opt_a").expect("explain_why succeeds");
    assert!(matches!(why, WhyExplanation::NotAdmitted { .. }));

    // Check why not
    let whynot = runtime
        .explain_why_not("opt_a")
        .expect("explain_why_not succeeds");
    assert!(matches!(whynot, WhyNotExplanation::RejectedByPolicy { .. }));
}

// ---------------------------------------------------------------------------
// 12. Integration with External Inputs
// ---------------------------------------------------------------------------

#[test]
fn test_functions_with_external_inputs() {
    let source = r#"
config Decision = Allowed | Blocked

input user_age: Int
input user_country: Str

fn is_of_age(age: Int, min_age: Int): Bool = age >= min_age
fn country_allowed(c: Str): Bool = match c == "US" {
  true => true
  false => c == "CA"
}
fn both(a: Bool, b: Bool): Bool = match a {
  true => b
  false => false
}

rule eligible() = both(is_of_age(user_age, 21), country_allowed(user_country))

propose allow(eligible) priority 10 when eligible == true = Allowed
propose block() priority 100 when true = Blocked
commit c from (allow, block)
"#;
    let p = plan(source);
    let json = r#"{
        "schema": "brix.input@1",
        "values": {
            "user_age": {"type": "int", "value": "25"},
            "user_country": {"type": "string", "value": "US"}
        }
    }"#;
    let snapshot = snapshot_from_json(json);

    let runtime = FiniteDecisionRuntime::build_with_inputs(&p, &snapshot).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());
    assert_eq!(run.decision.as_ref().unwrap().candidate, "allow");

    // Produce and verify audit bundle with inputs
    let prog_id = finite_decision_program_id(&p);
    let bundle = produce_finite_decision_audit_input_bundle_v1(&runtime, &run)
        .expect("bundle produces cleanly");
    let bundle_bytes = bundle.canon_bytes();
    let decoded = decode_audit_input_bundle_v1(&bundle_bytes, &AuditDecodeLimits::strict())
        .expect("bundle decodes cleanly");

    let report = check_finite_decision_audit_input_bundle_from_source_with_inputs_v1(
        source.as_bytes(),
        prog_id,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &decoded,
        &AuditDecodeLimits::strict(),
        &snapshot,
    )
    .expect("verification with inputs succeeds");

    assert_eq!(report.program, prog_id);
    assert_eq!(report.status(), "audit-bundle-verified");
}

// ---------------------------------------------------------------------------
// 13. Regression Tests (Review Fixes: ADR-0032)
// ---------------------------------------------------------------------------

#[test]
fn test_unused_invalid_declarations() {
    // Unused recursive helper must fail lowering
    let src_cycle = r#"
config Decision = Done
fn unused_rec(x: Int): Int = unused_rec(x)
rule r() = 42
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let mod_cycle = parse(src_cycle).expect("parses");
    let err = lower_finite_decision_plan(&mod_cycle, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::FunctionCycle { .. }
    ));

    // Unused helper reading rule fact must fail lowering
    let src_fact = r#"
config Decision = Done
rule r() = 42
fn unused_bad(x: Int): Int = x + r
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let mod_fact = parse(src_fact).expect("parses");
    let err2 = lower_finite_decision_plan(&mod_fact, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err2,
        FiniteDecisionLowerError::RuleFactReadInFunction { .. }
    ));

    // Unused helper with arity mismatch in its call
    let src_arity = r#"
config Decision = Done
fn foo(x: Int): Int = x
fn unused_bad(): Int = foo(1, 2)
rule r() = 42
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let mod_arity = parse(src_arity).expect("parses");
    let err3 = lower_finite_decision_plan(&mod_arity, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err3,
        FiniteDecisionLowerError::FunctionArityMismatch { .. }
    ));
}

#[test]
fn test_constructor_name_collision() {
    // Helper colliding with nullary variant
    let src_nullary = r#"
config Decision = Win | Lose
fn Win(): Int = 1
rule r() = 1
propose p(r) priority 10 when true = Lose
commit c from (p)
"#;
    let mod_nullary = parse(src_nullary).expect("parses");
    let err1 = lower_finite_decision_plan(&mod_nullary, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err1,
        FiniteDecisionLowerError::FunctionConstructorCollision { ref func, ref constructor }
        if func == "Win" && constructor == "Win"
    ));

    // Helper colliding with payload variant
    let src_payload = r#"
config Decision = Win(Int) | Lose
fn Win(x: Int): Int = x
rule r() = 1
propose p(r) priority 10 when true = Lose
commit c from (p)
"#;
    let mod_payload = parse(src_payload).expect("parses");
    let err2 = lower_finite_decision_plan(&mod_payload, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err2,
        FiniteDecisionLowerError::FunctionConstructorCollision { ref func, ref constructor }
        if func == "Win" && constructor == "Win"
    ));

    // Bool constructors are reserved lexer tokens, so they cannot be
    // redeclared as helper names at the surface syntax boundary.
    let src_bool = r#"
config Decision = Done
fn true(): Int = 1
rule r() = 1
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    assert!(parse(src_bool).is_err());
}

#[test]
fn test_constructor_local_binder_collision_precedence() {
    // In function-enabled lowering, parameter x shadows nullary variant x.
    let src = r#"
config Flag = x | y
config Decision = Done

fn id(x: Int): Int = x

rule r() = id(7)
propose p(r) priority 10 when r == 7 = Done
commit c from (p)
"#;
    let p = plan(src);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());
    assert_eq!(run.facts[0].value, L3ValueV2::Int(7));

    // Match binder also shadows nullary variant in function-enabled lowering.
    let src_match = r#"
config Flag = x | y
config Wrapper = Wrap(Int)
config Decision = Done

fn unwrap(w): Int = match w {
  Wrap(x) => x + 3
}

rule r() = unwrap(Wrap(10))
propose p(r) priority 10 when r == 13 = Done
commit c from (p)
"#;
    let p_match = plan(src_match);
    let runtime_match = FiniteDecisionRuntime::build(&p_match).expect("runtime builds");
    let run_match = runtime_match.run();
    assert!(run_match.is_selected());
    assert_eq!(run_match.facts[0].value, L3ValueV2::Int(13));
}

#[test]
fn test_duplicate_match_binders_rejected() {
    let src = r#"
config Pair = P(Int, Int)
config Decision = Done

fn swap(p): Int = match p {
  P(x, x) => x
}

rule r() = 1
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let module = parse(src).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::DuplicateMatchBinder(ref b) if b == "x"
    ));
}

#[test]
fn test_mixed_expression_and_call_depth_bounded() {
    let mut fns = String::new();
    for i in 0..70 {
        fns.push_str(&format!("fn f{i}(x: Int): Int = f{}(x + 1)\n", i + 1));
    }
    fns.push_str("fn f70(x: Int): Int = x\n");

    let source = format!(
        r#"
config Decision = Done
{fns}
rule r() = f0(0)
propose p(r) priority 10 when true = Done
commit c from (p)
"#
    );
    let p = plan(&source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_unknown());
    assert!(matches!(
        run.stop,
        FiniteDecisionStop::Unknown(
            FiniteDecisionUnknownReason::ExpressionEvaluationFault {
                context,
                fault: EvalFault::CallDepthExceeded { .. },
            }
        ) if context == "rule r"
    ));
}

#[test]
fn test_exponential_value_duplication_resource_exhausted() {
    let source = r#"
config Tree = Leaf | Node(Tree, Tree)
config Decision = Done

fn dup(t) = Node(t, t)
fn step1(t) = dup(dup(t))
fn step2(t) = step1(step1(t))
fn step3(t) = step2(step2(t))
fn step4(t) = step3(step3(t))

rule r() = step4(Leaf)
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_unknown());
    assert!(matches!(
        run.stop,
        FiniteDecisionStop::Unknown(
            FiniteDecisionUnknownReason::ExpressionEvaluationFault {
                context,
                fault: EvalFault::ResourceExhausted { .. },
            }
        ) if context == "rule r"
    ));
}

#[test]
fn test_runtime_work_bound() {
    let mut fns = String::new();
    for i in 0..15 {
        fns.push_str(&format!(
            "fn f{i}(x: Int): Int = f{}(f{}(x))\n",
            i + 1,
            i + 1
        ));
    }
    fns.push_str("fn f15(x: Int): Int = x + 1\n");

    let source = format!(
        r#"
config Decision = Done
{fns}
rule r() = f0(0)
propose p(r) priority 10 when true = Done
commit c from (p)
"#
    );
    let p = plan(&source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_unknown());
    assert!(matches!(
        run.stop,
        FiniteDecisionStop::Unknown(
            FiniteDecisionUnknownReason::ExpressionEvaluationFault {
                context,
                fault: EvalFault::ResourceExhausted { .. },
            }
        ) if context == "rule r"
    ));
}

#[test]
fn test_function_free_frozen_known_shipping_identity() {
    let mut module = parse(include_str!("../../../examples/shipping.brix")).unwrap();
    module
        .items
        .retain(|item| !matches!(item, brix_syntax::ast::Item::Show(_)));
    let p = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap();
    let prog_id = finite_decision_program_id(&p);
    assert_eq!(
        prog_id.to_hex(),
        "3a815590c807a8af7e7756d8f0edef99a4938e15830de282b24949fe88ba0d5e"
    );
}

#[test]
fn test_helper_contract_edit_pin_change() {
    let src1 = r#"
config Decision = Done
fn add(a: Int, b: Int): Int = a + b
rule r() = add(1, 2)
propose p(r) priority 10 when r == 3 = Done
commit c from (p)
"#;

    let src2 = r#"
config Decision = Done
fn add(a: Int @Derived, b: Int): Int = a + b
rule r() = add(1, 2)
propose p(r) priority 10 when r == 3 = Done
commit c from (p)
"#;

    let p1 = plan(src1);
    let p2 = plan(src2);
    let id1 = finite_decision_program_id(&p1);
    let id2 = finite_decision_program_id(&p2);
    assert_ne!(id1, id2);
    assert_ne!(
        finite_decision_program_preimage(&p1),
        finite_decision_program_preimage(&p2)
    );
}
