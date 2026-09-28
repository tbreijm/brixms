//! Generic (parameterized) configs as internal values in the finite-decision
//! lane (ADR-0042).
//!
//! **Name clash note (integrator):** the relations agent is adding `List<T>`
//! as a *built-in* collection type in this lane. These tests deliberately use
//! `Stack<T>`/`Tree<T>` for user-declared generic configs instead, so they do
//! not collide with that name.

use brix_lower::l3_v2::L3ValueV2;
use brix_lower::{
    finite_decision_program_id, finite_decision_program_preimage, lower_finite_decision_plan,
    FiniteDecisionLowerError, FiniteDecisionPlan, FiniteDecisionRuntime, FINITE_DECISION_PROFILE,
};
use brix_syntax::parse;

fn plan(source: &str) -> FiniteDecisionPlan {
    let module = parse(source).expect("fixture parses");
    lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).expect("fixture lowers")
}

#[test]
fn test_generic_sum_config_evaluates_as_internal_value() {
    let source = r#"
config Stack<T> = SNil | SCons(T, Stack<T>)
config Decision = Done

fn head_or(s, default) = match s {
  SNil => default
  SCons(x, rest) => x
}

rule top() = head_or(SCons(42, SNil), 0)
propose p(top) priority 10 when top == 42 = Done
commit c from (p)
"#;
    let p = plan(source);
    assert_eq!(
        p.generic_configs,
        vec![("Stack".to_string(), vec!["T".to_string()])]
    );
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected(), "expected selection, got {:?}", run.stop);
    assert_eq!(run.facts[0].value, L3ValueV2::Int(42));
}

#[test]
fn test_generic_config_recursion_over_stack_length() {
    let source = r#"
config Stack<T> = SNil | SCons(T, Stack<T>)
config Decision = Done

fn length(s) = match s {
  SNil => 0
  SCons(x, rest) => 1 + length(rest)
}

rule n() = length(SCons(1, SCons(2, SCons(3, SCons(4, SNil)))))
propose p(n) priority 10 when n == 4 = Done
commit c from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());
    assert_eq!(run.facts[0].value, L3ValueV2::Int(4));
}

#[test]
fn test_generic_record_config_evaluates_as_internal_value() {
    let source = r#"
config Box<T> = { value: T }
config Decision = Done

fn unbox(b) = b.value

rule v() = unbox(Box { value: 7 })
propose p(v) priority 10 when v == 7 = Done
commit c from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());
    assert_eq!(run.facts[0].value, L3ValueV2::Int(7));
}

#[test]
fn test_generic_config_declaration_is_bound_into_program_identity() {
    let generic_source = r#"
config Stack<T> = SNil | SCons(T, Stack<T>)
config Decision = Done
rule r() = 1
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let concrete_source = r#"
config Stack = SNil | SCons(Int, Stack)
config Decision = Done
rule r() = 1
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let generic_plan = plan(generic_source);
    let concrete_plan = plan(concrete_source);

    // Both configs have the identical arity-based shape (`SNil`/0,
    // `SCons`/2), so without the generic-configs preimage section the two
    // programs would be indistinguishable.
    assert_eq!(generic_plan.configs, concrete_plan.configs);
    assert_ne!(
        finite_decision_program_preimage(&generic_plan),
        finite_decision_program_preimage(&concrete_plan)
    );
    assert_ne!(
        finite_decision_program_id(&generic_plan),
        finite_decision_program_id(&concrete_plan)
    );

    // A program with no generic configs carries no generic-configs preimage
    // section at all, so `generic_configs` is empty (additive: identity for
    // every pre-existing, non-generic program is unaffected).
    assert!(concrete_plan.generic_configs.is_empty());
    assert_eq!(
        generic_plan.generic_configs,
        vec![("Stack".to_string(), vec!["T".to_string()])]
    );
}

#[test]
fn test_generic_config_still_refused_as_input_type_with_clear_diagnostic() {
    let source = r#"
config Stack<T> = SNil | SCons(T, Stack<T>)
config Decision = Done

input s: Stack

rule r() = 1
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    let FiniteDecisionLowerError::InvalidSchema { name, detail } = &err else {
        panic!("expected InvalidSchema, got {err:?}");
    };
    assert_eq!(name, "Stack");
    assert!(
        detail.contains("erased"),
        "diagnostic should explain type-parameter erasure, got: {detail}"
    );
}

#[test]
fn test_generic_config_still_refused_as_helper_contract_with_clear_diagnostic() {
    // A helper contract naming a declared generic config is caught by the
    // same reachable-schema pass an `input` declaration is (both a parameter
    // and a return-type annotation are schema roots), so the diagnostic is
    // `InvalidSchema`, with a message naming the reason (erasure) rather than
    // a bare "unsupported".
    let source = r#"
config Stack<T> = SNil | SCons(T, Stack<T>)
config Decision = Done

fn head(s: Stack<Int>): Int = match s {
  SNil => 0
  SCons(x, rest) => x
}

rule r() = head(SCons(1, SNil))
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    let FiniteDecisionLowerError::InvalidSchema { name, detail } = &err else {
        panic!("expected InvalidSchema, got {err:?}");
    };
    assert_eq!(name, "Stack");
    assert!(
        detail.contains("erased"),
        "diagnostic should explain type-parameter erasure, got: {detail}"
    );
}

#[test]
fn test_undeclared_generic_type_in_contract_gives_clear_diagnostic() {
    // A contract naming a type that was never declared at all as a config —
    // distinct from a *declared* generic config — is refused directly by
    // `parse_contract`, since it never becomes a schema root.
    let source = r#"
config Decision = Done

fn head(s: Bogus<Int>): Int = 0

rule r() = 1
propose p(r) priority 10 when true = Done
commit c from (p)
"#;
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    let FiniteDecisionLowerError::UnsupportedContractType { ty, detail } = &err else {
        panic!("expected UnsupportedContractType, got {err:?}");
    };
    assert_eq!(ty, "Bogus<...>");
    assert!(
        detail.contains("erased"),
        "diagnostic should explain type-parameter erasure, got: {detail}"
    );
}
