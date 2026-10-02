//! Numeric execution faults and explicit-domain behavior.
use brix_lower::l3_v2::{
    eval, lower_l3_plan_v2, ArithOpV2, CmpOpV2, EvalEnv, EvalFault, L3ExprV2, L3PlanItemV2,
    L3ValueV2, NumericBuiltinV2, L3_PROFILE_MARKER_V2,
};

fn ctor(op: NumericBuiltinV2, text: &str) -> L3ExprV2 {
    L3ExprV2::NumericBuiltin(op, vec![L3ExprV2::Str(text.into())])
}
fn f64_expr(text: &str) -> L3ExprV2 {
    ctor(NumericBuiltinV2::F64, text)
}
fn decimal(text: &str) -> L3ExprV2 {
    ctor(NumericBuiltinV2::Decimal, text)
}
fn arith(op: ArithOpV2, a: L3ExprV2, b: L3ExprV2) -> L3ExprV2 {
    L3ExprV2::Arith(op, Box::new(a), Box::new(b))
}
fn run(expr: &L3ExprV2) -> Result<L3ValueV2, EvalFault> {
    eval(expr, &EvalEnv::new())
}

#[test]
fn decimal_is_exact_while_float_has_binary64_rounding() {
    let actual = run(&arith(ArithOpV2::Add, decimal("0.1"), decimal("0.2"))).unwrap();
    assert_eq!(actual, run(&decimal("0.3")).unwrap());
    let actual = run(&arith(ArithOpV2::Add, f64_expr("0.1"), f64_expr("0.2"))).unwrap();
    assert_ne!(actual, run(&f64_expr("0.3")).unwrap());
}

#[test]
fn same_domain_arithmetic_and_comparisons_execute() {
    for construct in [f64_expr, decimal] {
        for (op, expected) in [
            (ArithOpV2::Add, "9"),
            (ArithOpV2::Sub, "3"),
            (ArithOpV2::Mul, "18"),
            (ArithOpV2::Div, "2"),
        ] {
            assert_eq!(
                run(&arith(op, construct("6"), construct("3"))).unwrap(),
                run(&construct(expected)).unwrap()
            );
        }
        for (op, expected) in [
            (CmpOpV2::Lt, true),
            (CmpOpV2::Le, true),
            (CmpOpV2::Gt, false),
            (CmpOpV2::Ge, false),
            (CmpOpV2::Eq, false),
            (CmpOpV2::Ne, true),
        ] {
            assert_eq!(
                run(&L3ExprV2::Cmp(
                    op,
                    Box::new(construct("-1.5")),
                    Box::new(construct("0.25"))
                ))
                .unwrap(),
                L3ValueV2::Bool(expected)
            );
        }
    }
}

#[test]
fn numeric_faults_never_produce_values() {
    for expr in [
        f64_expr("NaN"),
        f64_expr("inf"),
        decimal("1e3"),
        arith(ArithOpV2::Div, f64_expr("1"), f64_expr("0")),
        arith(ArithOpV2::Mul, f64_expr("1e308"), f64_expr("2")),
        arith(ArithOpV2::Div, decimal("1"), decimal("0")),
        arith(ArithOpV2::Div, decimal("1"), decimal("3")),
    ] {
        assert!(matches!(run(&expr), Err(EvalFault::Numeric(_))), "{expr:?}");
    }
    for expr in [
        arith(ArithOpV2::Add, decimal("1"), f64_expr("1")),
        arith(ArithOpV2::Mul, f64_expr("1"), L3ExprV2::Int(2)),
        arith(ArithOpV2::Div, L3ExprV2::Int(4), L3ExprV2::Int(2)),
    ] {
        assert!(matches!(run(&expr), Err(EvalFault::OperandShape(_))));
    }
}

#[test]
fn decimal_rounding_is_explicit_and_negative_ties_are_even() {
    for (mode, expected) in [
        ("floor", "-1.3"),
        ("ceil", "-1.2"),
        ("half_even", "-1.2"),
        ("trunc", "-1.2"),
    ] {
        let expression = L3ExprV2::NumericBuiltin(
            NumericBuiltinV2::DecimalDiv,
            vec![
                decimal("-5"),
                decimal("4"),
                L3ExprV2::Int(1),
                L3ExprV2::Str(mode.into()),
            ],
        );
        assert_eq!(run(&expression).unwrap(), run(&decimal(expected)).unwrap());
    }
    for scale in [-1, 19, 256] {
        let expression = L3ExprV2::NumericBuiltin(
            NumericBuiltinV2::DecimalDiv,
            vec![
                decimal("1"),
                decimal("3"),
                L3ExprV2::Int(scale),
                L3ExprV2::Str("half_even".into()),
            ],
        );
        assert!(matches!(run(&expression), Err(EvalFault::Numeric(_))));
    }
}

#[test]
fn construction_conversion_and_negation_are_explicit() {
    for (op, construct, negate) in [
        (
            NumericBuiltinV2::F64FromInt,
            f64_expr as fn(&str) -> L3ExprV2,
            NumericBuiltinV2::F64Neg,
        ),
        (
            NumericBuiltinV2::DecimalFromInt,
            decimal,
            NumericBuiltinV2::DecimalNeg,
        ),
    ] {
        let expr = L3ExprV2::NumericBuiltin(op, vec![L3ExprV2::Int(42)]);
        assert_eq!(run(&expr).unwrap(), run(&construct("42")).unwrap());
        let expr = L3ExprV2::NumericBuiltin(negate, vec![expr]);
        assert_eq!(run(&expr).unwrap(), run(&construct("-42")).unwrap());
    }
    assert_eq!(run(&f64_expr("-0")).unwrap(), run(&f64_expr("0")).unwrap());
    assert_eq!(run(&decimal("1.00")).unwrap(), run(&decimal("1")).unwrap());
}

#[test]
fn source_constructors_and_division_lower_and_run() {
    let module = brix_syntax::parse("let x = decimal(\"1\") / decimal(\"8\")\n").unwrap();
    let plan = lower_l3_plan_v2(&module, L3_PROFILE_MARKER_V2).unwrap();
    let L3PlanItemV2::Let { value, .. } = &plan.items[0] else {
        panic!("expected let")
    };
    assert_eq!(run(value).unwrap(), run(&decimal("0.125")).unwrap());
    let module = brix_syntax::parse("let x = 1 / 8\n").unwrap();
    assert!(lower_l3_plan_v2(&module, L3_PROFILE_MARKER_V2).is_err());
    let module = brix_syntax::parse("let x = f64(\"1\", \"2\")\n").unwrap();
    assert!(lower_l3_plan_v2(&module, L3_PROFILE_MARKER_V2).is_err());
}

#[test]
fn malformed_ir_builtin_is_a_shape_fault() {
    for op in NumericBuiltinV2::ALL {
        assert!(matches!(
            run(&L3ExprV2::NumericBuiltin(op, vec![])),
            Err(EvalFault::OperandShape(_))
        ));
    }
}

#[test]
fn equality_does_not_cross_numeric_domains() {
    for op in [CmpOpV2::Eq, CmpOpV2::Ne] {
        for (a, b) in [
            (decimal("1"), f64_expr("1")),
            (L3ExprV2::Int(1), decimal("1")),
            (f64_expr("1"), L3ExprV2::Int(1)),
        ] {
            assert!(matches!(
                run(&L3ExprV2::Cmp(op, Box::new(a), Box::new(b))),
                Err(EvalFault::OperandShape(_))
            ));
        }
    }
}
