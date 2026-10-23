//! Checkpoint 3 small expression forms in the finite-decision lane:
//! `only(xs)`, a final wildcard `match` arm, and `[]: List<T>` ascription.
//!
//! Every case drives the shared evaluator through `FiniteDecisionRuntime`, the
//! same path `check`/`run`/`why` use, rather than a private interpreter.

use brix_lower::l3_v2::{EvalFault, L3ValueV2};
use brix_lower::{
    finite_decision_program_id, lower_finite_decision_plan, FiniteDecisionRuntime,
    FiniteDecisionStop, FiniteDecisionUnknownReason, FINITE_DECISION_PROFILE,
};
use brix_syntax::parse;

fn try_plan(
    source: &str,
) -> Result<brix_lower::FiniteDecisionPlan, brix_lower::FiniteDecisionLowerError> {
    let module = parse(source).expect("fixture parses");
    lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
}

fn plan(source: &str) -> brix_lower::FiniteDecisionPlan {
    try_plan(source).expect("fixture lowers")
}

fn refusal(source: &str) -> String {
    format!(
        "{:?}",
        try_plan(source).expect_err("fixture must be refused")
    )
}

fn selected(source: &str) -> String {
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(
        run.is_selected(),
        "expected a selection, got {:?}",
        run.stop
    );
    run.decision.as_ref().unwrap().candidate.clone()
}

fn fault(source: &str) -> EvalFault {
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    match run.stop {
        FiniteDecisionStop::Unknown(FiniteDecisionUnknownReason::ExpressionEvaluationFault {
            fault,
            ..
        }) => fault,
        other => panic!("expected an evaluation fault, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// only(xs)
// ---------------------------------------------------------------------------

#[test]
fn only_returns_the_single_element() {
    let source = r#"
config Decision = Ship | Hold
rule r() = only(["a"])
propose p(r) priority 10 when r == "a" = Ship
commit c from (p)
"#;
    assert_eq!(selected(source), "p");
}

#[test]
fn only_of_an_empty_list_is_a_typed_fault_with_cardinality() {
    let source = r#"
config Decision = Ship | Hold
rule r() = only([]: List<Str>)
propose p(r) priority 10 when true = Ship
commit c from (p)
"#;
    assert_eq!(fault(source), EvalFault::NotSingleton { found: 0 });
}

#[test]
fn only_of_several_elements_reports_how_many() {
    let source = r#"
config Decision = Ship | Hold
rule r() = only(["a", "b", "c"])
propose p(r) priority 10 when true = Ship
commit c from (p)
"#;
    assert_eq!(fault(source), EvalFault::NotSingleton { found: 3 });
}

#[test]
fn only_fault_does_not_select_default() {
    // The faulty fact `r` feeds `p`. A lower-priority candidate with an always
    // true guard exists as the would-be default. The fault must stop the run;
    // it must never be read as "no match, fall through to the default".
    let source = r#"
config Decision = Ship | Hold
rule r() = only(["a", "b"])
propose p(r) priority 10 when r == "a" = Ship
propose fallback() priority 1 when true = Hold
commit c from (p, fallback)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(
        !run.is_selected(),
        "a faulty fact must not select any candidate"
    );
    assert!(run.is_unknown());
}

#[test]
fn only_preserves_a_nominal_element_without_coercion() {
    let source = r#"
config Kind = Alpha | Beta
config Decision = Ship | Hold
rule r() = only([Beta])
propose p(r) priority 10 when match r { Alpha => false  Beta => true } = Ship
commit c from (p)
"#;
    assert_eq!(selected(source), "p");
}

#[test]
fn only_requires_a_list_operand() {
    let source = r#"
config Decision = Ship
rule r() = only(1)
propose p(r) priority 10 when true = Ship
commit c from (p)
"#;
    assert!(matches!(fault(source), EvalFault::OperandShape(_)));
}

#[test]
fn only_is_a_reserved_name_not_a_shadowable_helper() {
    let source = r#"
config Decision = Ship
fn only(x: Int): Int = x
rule r() = 1
propose p(r) priority 10 when true = Ship
commit c from (p)
"#;
    assert!(try_plan(source).is_err());
}

#[test]
fn only_value_is_the_element_value() {
    // The shared evaluator, directly: the produced value is the element.
    use brix_lower::l3_v2::{eval, EvalEnv, L3ExprV2};
    let expr = L3ExprV2::Only(Box::new(L3ExprV2::ListLit(vec![L3ExprV2::Int(7)])));
    assert_eq!(eval(&expr, &EvalEnv::new()), Ok(L3ValueV2::Int(7)));
    let two = L3ExprV2::Only(Box::new(L3ExprV2::ListLit(vec![
        L3ExprV2::Int(7),
        L3ExprV2::Int(8),
    ])));
    assert_eq!(
        eval(&two, &EvalEnv::new()),
        Err(EvalFault::NotSingleton { found: 2 })
    );
}

// ---------------------------------------------------------------------------
// final wildcard arm
// ---------------------------------------------------------------------------

const Q: &str = r#"
config Q = Volume | Trend | Rank | Share
config Decision = Ship | Hold
let q = Trend
"#;

#[test]
fn a_final_wildcard_covers_the_unlisted_constructors() {
    let source = format!(
        "{Q}rule r() = match q {{ Volume => 1  Trend => 2  _ => 9 }}\n\
         propose p(r) priority 10 when r == 2 = Ship\ncommit c from (p)\n"
    );
    assert_eq!(selected(&source), "p");

    let source = format!(
        "{Q}rule r() = match Share {{ Volume => 1  _ => 9 }}\n\
         propose p(r) priority 10 when r == 9 = Ship\ncommit c from (p)\n"
    );
    assert_eq!(selected(&source), "p");
}

#[test]
fn an_explicit_arm_wins_over_the_wildcard_that_follows() {
    let source = format!(
        "{Q}rule r() = match Volume {{ Volume => 1  _ => 9 }}\n\
         propose p(r) priority 10 when r == 1 = Ship\ncommit c from (p)\n"
    );
    assert_eq!(selected(&source), "p");
}

#[test]
fn a_wildcard_that_is_not_last_is_refused() {
    let source = format!(
        "{Q}rule r() = match q {{ _ => 9  Volume => 1 }}\n\
         propose p(r) priority 10 when true = Ship\ncommit c from (p)\n"
    );
    assert!(refusal(&source).contains("WildcardArmNotLast"));
}

#[test]
fn a_variable_catch_all_is_still_refused() {
    let source = format!(
        "{Q}rule r() = match q {{ Volume => 1  other => 9 }}\n\
         propose p(r) priority 10 when true = Ship\ncommit c from (p)\n"
    );
    assert!(refusal(&source).contains("DefaultArmNotAllowed"));
}

#[test]
fn a_wildcard_after_a_complete_match_is_an_unreachable_arm() {
    let source = format!(
        "{Q}rule r() = match q {{ Volume => 1  Trend => 2  Rank => 3  Share => 4  _ => 9 }}\n\
         propose p(r) priority 10 when true = Ship\ncommit c from (p)\n"
    );
    assert!(refusal(&source).contains("UnreachableWildcardArm"));
}

#[test]
fn a_wildcard_does_not_hide_a_non_sum_scrutinee() {
    let source = format!(
        "{Q}rule r() = match 5 {{ Volume => 1  _ => 9 }}\n\
         propose p(r) priority 10 when true = Ship\ncommit c from (p)\n"
    );
    assert!(matches!(fault(&source), EvalFault::OperandShape(_)));
}

#[test]
fn a_wildcard_arm_does_not_change_the_identity_of_plans_without_one() {
    let exhaustive = format!(
        "{Q}rule r() = match q {{ Volume => 1  Trend => 2  Rank => 3  Share => 4 }}\n\
         propose p(r) priority 10 when true = Ship\ncommit c from (p)\n"
    );
    let with_wildcard = format!(
        "{Q}rule r() = match q {{ Volume => 1  Trend => 2  Rank => 3  _ => 4 }}\n\
         propose p(r) priority 10 when true = Ship\ncommit c from (p)\n"
    );
    assert_ne!(
        finite_decision_program_id(&plan(&exhaustive)),
        finite_decision_program_id(&plan(&with_wildcard)),
        "a wildcard is a different program, not an alias"
    );
}

// ---------------------------------------------------------------------------
// ascription
// ---------------------------------------------------------------------------

#[test]
fn an_ascribed_empty_list_is_the_same_program_as_a_bare_one() {
    let plain = r#"
config Decision = Ship
rule r() = len([])
propose p(r) priority 10 when r == 0 = Ship
commit c from (p)
"#;
    let typed = r#"
config Decision = Ship
rule r() = len([]: List<Str>)
propose p(r) priority 10 when r == 0 = Ship
commit c from (p)
"#;
    assert_eq!(selected(typed), "p");
    assert_ne!(
        finite_decision_program_id(&plan(plain)),
        finite_decision_program_id(&plan(typed)),
        "the annotation is retained for runtime checking"
    );
}

#[test]
fn a_nonempty_list_is_checked_against_its_annotation() {
    let ok = r#"
config Decision = Ship
rule r() = len(["a", "b"]: List<Str>)
propose p(r) priority 10 when r == 2 = Ship
commit c from (p)
"#;
    assert_eq!(selected(ok), "p");

    let bad = r#"
config Decision = Ship
rule r() = len([1, 2]: List<Str>)
propose p(r) priority 10 when true = Ship
commit c from (p)
"#;
    assert!(refusal(bad).contains("AscriptionMismatch"));
}

#[test]
fn an_ascription_that_is_not_a_list_is_refused() {
    let source = r#"
config Decision = Ship
rule r() = 1: List<Int>
propose p(r) priority 10 when true = Ship
commit c from (p)
"#;
    assert!(refusal(source).contains("AscriptionMismatch"));

    let source = r#"
config Decision = Ship
rule r() = []: Int
propose p(r) priority 10 when true = Ship
commit c from (p)
"#;
    assert!(refusal(source).contains("AscriptionMismatch"));
}
