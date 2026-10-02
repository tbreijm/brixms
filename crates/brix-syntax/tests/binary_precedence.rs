use brix_syntax::ast::{BinOp, Expr, Item, LetDecl};
use brix_syntax::parse;

fn let_value(source: &str) -> Result<Expr, brix_syntax::ParseError> {
    let module = parse(source)?;
    let Item::Let(LetDecl { value, .. }) = &module.items[0] else {
        panic!("expected a let binding");
    };
    Ok(value.clone())
}

#[test]
fn witness_composition_is_left_associative_below_boolean_and_arithmetic_operators() {
    let value = let_value("let result = a then b and c || d && e + f * g").expect("parse");
    let Expr::Bin {
        op: BinOp::And,
        lhs,
        rhs,
    } = &value
    else {
        panic!("expected left-associative witness composition at the root");
    };

    assert!(matches!(
        **lhs,
        Expr::Bin {
            op: BinOp::Then,
            ..
        }
    ));
    let Expr::Bin {
        op: BinOp::OrOr,
        lhs: or_lhs,
        rhs: or_rhs,
    } = &**rhs
    else {
        panic!("expected || to bind inside the witness composition");
    };
    assert!(matches!(**or_lhs, Expr::Var(ref name) if name == "c"));
    assert!(matches!(
        **or_rhs,
        Expr::Bin {
            op: BinOp::AndAnd,
            rhs: ref arithmetic,
            ..
        } if matches!(**arithmetic, Expr::Bin { op: BinOp::Add, rhs: ref product, .. }
            if matches!(**product, Expr::Bin { op: BinOp::Mul, .. }))
    ));

    let value = let_value("let result = a then b and c").expect("parse");
    assert!(matches!(
        value,
        Expr::Bin {
            op: BinOp::And,
            lhs,
            rhs,
        } if matches!(*lhs, Expr::Bin { op: BinOp::Then, .. })
            && matches!(*rhs, Expr::Var(ref name) if name == "c")
    ));
}

#[test]
fn separate_comparisons_can_be_combined_with_short_circuit_and() {
    let value = let_value("let result = a < b && c < d").expect("independent comparisons parse");
    assert!(matches!(
        value,
        Expr::Bin {
            op: BinOp::AndAnd,
            lhs,
            rhs,
        } if matches!(*lhs, Expr::Bin { op: BinOp::Lt, .. })
            && matches!(*rhs, Expr::Bin { op: BinOp::Lt, .. })
    ));
}

#[test]
fn unparenthesized_comparison_chains_are_rejected() {
    let error = let_value("let result = a < b == c").expect_err("comparison chain must fail");
    assert!(error.to_string().contains("do not chain"), "{error}");
}

#[test]
fn explicit_grouping_allows_a_comparison_result_to_be_compared() {
    let value = let_value("let result = (a < b) == c").expect("grouped comparison parses");
    assert!(matches!(
        value,
        Expr::Bin {
            op: BinOp::Eq,
            lhs,
            rhs,
        } if matches!(*lhs, Expr::Bin { op: BinOp::Lt, .. })
            && matches!(*rhs, Expr::Var(ref name) if name == "c")
    ));
}
