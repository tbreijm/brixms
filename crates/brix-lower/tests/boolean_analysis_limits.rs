//! The static pass must refuse bounded work without executing helper bodies.

use brix_lower::finite_decision::{
    lower_finite_decision_plan, FiniteDecisionLowerError, FINITE_DECISION_PROFILE,
};
use brix_syntax::parse;

#[test]
fn boolean_analysis_bounds_helper_expansion() {
    // Avoid the reserved f64 constructor in generated helper names.
    let mut source = "fn helper0() = true\n".to_string();
    for i in 1..25 {
        source.push_str(&format!(
            "fn helper{i}() = helper{}() && helper{}()\n",
            i - 1,
            i - 1
        ));
    }
    source.push_str("propose p() priority 1 when false && helper24() = true\ncommit c from (p)");
    let module = parse(&source).unwrap();
    assert_eq!(
        lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE),
        Err(FiniteDecisionLowerError::BooleanTypeAnalysisLimit)
    );
}

#[test]
fn boolean_analysis_bounds_helper_depth() {
    // Avoid the reserved f64 constructor in generated helper names.
    let mut source = "fn helper0() = true\n".to_string();
    for i in 1..150 {
        source.push_str(&format!("fn helper{i}() = helper{}()\n", i - 1));
    }
    source.push_str("propose p() priority 1 when false && helper149() = true\ncommit c from (p)");
    let module = parse(&source).unwrap();
    assert_eq!(
        lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE),
        Err(FiniteDecisionLowerError::BooleanTypeAnalysisLimit)
    );
}
