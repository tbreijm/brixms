//! World scalar execution must share existing numeric and refusal semantics.
use brix_kb::world::network::{eval_expr, IntermediateTuple, Value};
use brix_lower::world_expr::CompiledWorldExpr;
use brix_syntax::ast::{BinOp, Callable, Expr, Param, Ty};
use std::collections::{BTreeMap, BTreeSet};
fn bin(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
    Expr::Bin {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    }
}
fn num(v: i64) -> Expr {
    Expr::Num(v.to_string())
}
fn call(name: &str, args: Vec<Expr>) -> Expr {
    Expr::Call {
        func: name.into(),
        args,
    }
}
fn evaluate(expr: Expr) -> Result<Value, brix_kb::world::WorldError> {
    eval_expr(&expr, &IntermediateTuple::new(), &BTreeMap::new())
}

#[test]
fn schema_decoding_preserves_reserved_strings() {
    let str_ty = Ty::Named("Str".into());
    for text in ["001", "true", "false", "", "9223372036854775808"] {
        assert_eq!(
            Value::from_typed_bytes(text.as_bytes(), &str_ty).unwrap(),
            Value::Str(text.into())
        );
    }
    assert_eq!(
        Value::from_typed_bytes(b"001", &Ty::Named("Int".into())).unwrap(),
        Value::Int(1)
    );
    assert!(Value::from_typed_bytes(b"1", &Ty::Named("Bool".into())).is_err());
    assert!(Value::from_typed_bytes(&[255], &str_ty).is_err());
    assert!(Value::Str("true".into()).as_bool().is_err());
    assert!(Value::Int(1).as_bool().is_err());
    assert!(Value::Bool(true).as_bool().unwrap());
}

#[test]
fn numeric_faults_are_refusals_and_never_saturated_values() {
    assert!(evaluate(bin(BinOp::Add, num(i64::MAX), num(1)))
        .unwrap_err()
        .to_string()
        .contains("overflow"));
    assert!(evaluate(bin(BinOp::Mul, num(i64::MAX), num(2))).is_err());
    assert!(evaluate(call("div_floor", vec![num(1), num(0)]))
        .unwrap_err()
        .to_string()
        .contains("zero"));
    assert!(evaluate(call("div_floor", vec![num(i64::MIN), num(-1)])).is_err());
    assert!(evaluate(bin(BinOp::Add, Expr::Str("001".into()), num(1))).is_err());
    assert!(evaluate(bin(
        BinOp::AndAnd,
        Expr::Str("true".into()),
        Expr::Bool(true)
    ))
    .is_err());
    assert_eq!(
        evaluate(bin(
            BinOp::AndAnd,
            Expr::Bool(false),
            call("div_floor", vec![num(1), num(0)])
        ))
        .unwrap(),
        Value::Bool(false)
    );
}

#[test]
fn explicit_f64_and_decimal_use_shared_arithmetic() {
    let decimal = |s: &str| call("decimal", vec![Expr::Str(s.into())]);
    let float = |s: &str| call("f64", vec![Expr::Str(s.into())]);
    assert_eq!(
        evaluate(bin(BinOp::Add, decimal("0.1"), decimal("0.2"))).unwrap(),
        Value::Decimal(brix_canon::decimal_parse("0.3").unwrap())
    );
    assert!(evaluate(bin(BinOp::Div, decimal("1"), decimal("3"))).is_err());
    assert!(evaluate(bin(BinOp::Add, decimal("1"), float("1"))).is_err());
    assert!(evaluate(float("NaN")).is_err());
    assert_eq!(
        evaluate(bin(BinOp::Add, float("1.5"), float("2.5"))).unwrap(),
        Value::F64("4".parse().unwrap())
    );
}

#[test]
fn missing_fields_and_helper_contracts_fail_closed() {
    let mut tuple = IntermediateTuple::new();
    tuple.insert("a.id", Value::Str("001".into()));
    tuple.insert("b.id", Value::Str("002".into()));
    let field =
        |binding: &str, name: &str| Expr::Field(Box::new(Expr::Var(binding.into())), name.into());
    assert_eq!(
        eval_expr(&field("a", "id"), &tuple, &BTreeMap::new()).unwrap(),
        Value::Str("001".into())
    );
    assert!(eval_expr(&field("c", "id"), &tuple, &BTreeMap::new()).is_err());
    assert!(eval_expr(&field("a", "missing"), &tuple, &BTreeMap::new()).is_err());
    assert!(eval_expr(&Expr::Var("id".into()), &tuple, &BTreeMap::new()).is_err());
    let helpers = BTreeMap::from([(
        "inc".into(),
        Callable {
            name: "inc".into(),
            params: vec![Param {
                name: "x".into(),
                ty: Some(Ty::Named("Int".into())),
            }],
            ret: Some(Ty::Named("Int".into())),
            body: bin(BinOp::Add, Expr::Var("x".into()), num(1)),
            params_declared: true,
        },
    )]);
    assert!(eval_expr(&call("inc", vec![Expr::Str("1".into())]), &tuple, &helpers).is_err());
    assert!(eval_expr(&call("inc", vec![]), &tuple, &helpers).is_err());
    let compiled = CompiledWorldExpr::new(
        &call("inc", vec![Expr::Var("n".into())]),
        &helpers,
        &BTreeSet::from(["n".into()]),
    )
    .unwrap();
    for n in [1, 50, 100] {
        assert_eq!(
            compiled
                .eval(&BTreeMap::from([(
                    "n".into(),
                    brix_lower::l3_v2::L3ValueV2::Int(n)
                )]))
                .unwrap(),
            brix_lower::l3_v2::L3ValueV2::Int(n + 1)
        );
    }
}

#[test]
fn named_record_helper_checks_nominal_identity_and_field_types() {
    use brix_lower::l3_v2::{L3Schema, L3SchemaBody, L3SchemaType, L3ValueV2 as V};
    use std::sync::Arc;
    let schema = L3Schema {
        name: "Order".into(),
        body: L3SchemaBody::Record(BTreeMap::from([("amount".into(), L3SchemaType::Decimal)])),
    };
    let helpers = BTreeMap::from([(
        "total".into(),
        Callable {
            name: "total".into(),
            params: vec![Param {
                name: "o".into(),
                ty: Some(Ty::Named("Order".into())),
            }],
            ret: Some(Ty::Named("Decimal".into())),
            body: Expr::Field(Box::new(Expr::Var("o".into())), "amount".into()),
            params_declared: true,
        },
    )]);
    let compiled = CompiledWorldExpr::with_schemas(
        &call("total", vec![Expr::Var("order".into())]),
        &helpers,
        &BTreeSet::from(["order".into()]),
        Arc::new(BTreeMap::from([("Order".into(), schema)])),
    )
    .unwrap();
    let amount = brix_canon::decimal_parse("0.25").unwrap();
    let binding = |nominal: &str, value: V| {
        BTreeMap::from([(
            "order".into(),
            V::Record {
                nominal_config: nominal.into(),
                fields: vec![("amount".into(), value)],
            },
        )])
    };
    assert_eq!(
        compiled
            .eval(&binding("Order", V::Decimal(amount)))
            .unwrap(),
        V::Decimal(amount)
    );
    assert!(compiled
        .eval(&binding("Other", V::Decimal(amount)))
        .is_err());
    assert!(compiled
        .eval(&binding("Order", V::Str("0.25".into())))
        .is_err());
}
