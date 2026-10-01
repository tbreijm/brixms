//! Numeric contracts are checked before lazy execution can hide a type error.
use brix_lower::finite_decision::{
    finite_decision_program_id, lower_finite_decision_plan, FiniteDecisionLowerError,
    FINITE_DECISION_PROFILE,
};
use brix_lower::l3_v2::L3V2LowerError;

fn lower(
    source: &str,
) -> Result<brix_lower::finite_decision::FiniteDecisionPlan, FiniteDecisionLowerError> {
    lower_finite_decision_plan(
        &brix_syntax::parse(source).unwrap(),
        FINITE_DECISION_PROFILE,
    )
}

#[test]
fn integer_division_through_inputs_helpers_and_bindings_is_rejected() {
    for source in [
        "input n: Int\npropose p() priority 1 when true = n / 2\ncommit c from (p)",
        "fn half(n: Int): Int = n / 2\npropose p() priority 1 when true = half(4)\ncommit c from (p)",
        "fn half(n) = n / 2\npropose p() priority 1 when true = half(4)\ncommit c from (p)",
        "let n = 4\npropose p() priority 1 when false && n / 2 == 2 = true\ncommit c from (p)",
        "input ns: List<Int> max 4\ndecide d for n in ns { propose p() priority 1 when true = n / 2 }",
    ] {
        assert!(matches!(lower(source), Err(FiniteDecisionLowerError::ExprError(L3V2LowerError::DivisionNotAllowed))), "{source}");
    }
}

#[test]
fn numeric_domains_remain_distinct_through_helpers_and_structured_contracts() {
    let source = r#"
config Measurement = { value: F64, cost: Decimal }
input m: Measurement
fn rate(m: Measurement): F64 = m.value / f64("2")
fn cost(m: Measurement): Decimal = m.cost / decimal("2")
propose p() priority 1 when rate(m) > f64("0") = cost(m)
commit c from (p)
"#;
    let plan = lower(source).unwrap();
    let pin = finite_decision_program_id(&plan);
    let changed = lower(&source.replace("f64(\"2\")", "f64(\"3\")")).unwrap();
    assert_ne!(pin, finite_decision_program_id(&changed));
}

#[test]
fn known_invalid_numeric_operands_are_rejected_even_when_skipped() {
    for expr in [
        "f64(1)",
        "decimal(true)",
        "f64_from_int(decimal(\"1\"))",
        "f64(\"1\") + decimal(\"1\")",
        "decimal_div(decimal(\"1\"), 3, 2, \"floor\")",
    ] {
        let source = format!(
            "propose p() priority 1 when false && ({expr} == f64(\"1\")) = true\ncommit c from (p)"
        );
        assert!(
            matches!(
                lower(&source),
                Err(FiniteDecisionLowerError::NumericOperandType { .. })
            ),
            "{source}"
        );
    }
    // Numeric constructors must not hide nested Boolean type errors.
    assert!(
        lower("propose p() priority 1 when true = f64(false && 1)\ncommit c from (p)").is_err()
    );
}

#[test]
fn skipped_numeric_faults_are_not_evaluated_by_static_checks() {
    for expr in [
        "f64(\"NaN\")",
        "f64(\"1\") / f64(\"0\")",
        "decimal(\"1\") / decimal(\"3\")",
    ] {
        let source = format!(
            "propose p() priority 1 when false && ({expr} == f64(\"1\")) = true\ncommit c from (p)"
        );
        assert!(lower(&source).is_ok(), "{source}");
    }
}

#[test]
fn numeric_type_and_operation_names_cannot_be_shadowed() {
    for declaration in [
        "config F64 = Fake",
        "config Decimal = Fake",
        "fn f64(x) = x",
        "fn decimal_div(a,b,c,d) = a",
    ] {
        let source =
            format!("{declaration}\npropose p() priority 1 when true = 1\ncommit c from (p)");
        assert!(
            matches!(
                lower(&source),
                Err(FiniteDecisionLowerError::ReservedOperationName { .. })
            ),
            "{source}"
        );
    }
}
