//! Runtime list checks must preserve numeric types and participate in identity.

use brix_lower::{
    finite_decision_program_id, lower_finite_decision_plan, FiniteDecisionPlan,
    FiniteDecisionRuntime, FINITE_DECISION_PROFILE,
};

fn plan(annotation: &str) -> FiniteDecisionPlan {
    let source = format!(
        "config Decision = Ship\n\
         rule r() = len([decimal(\"1\")]{annotation})\n\
         propose p(r) priority 10 when r == 1 = Ship\n\
         commit c from (p)\n"
    );
    lower_finite_decision_plan(&brix_syntax::parse(&source).unwrap(), FINITE_DECISION_PROFILE)
        .expect("valid decimal list must lower")
}

#[test]
fn correctly_ascribed_decimal_list_runs() {
    let plan = plan(": List<Decimal>");
    let run = FiniteDecisionRuntime::build(&plan).unwrap().run();
    assert!(run.is_selected(), "valid annotation faulted: {:?}", run.stop);
}

#[test]
fn programs_with_different_annotation_behavior_cannot_share_identity() {
    let plain = plan("");
    let typed = plan(": List<Decimal>");
    let plain_run = FiniteDecisionRuntime::build(&plain).unwrap().run();
    let typed_run = FiniteDecisionRuntime::build(&typed).unwrap().run();
    if plain_run.is_selected() != typed_run.is_selected() {
        assert_ne!(
            finite_decision_program_id(&plain),
            finite_decision_program_id(&typed),
            "selected and faulting programs must not share their canonical identity"
        );
    }
}
