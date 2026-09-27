use brix_syntax::ast::*;
use brix_syntax::parse;

#[test]
fn test_parse_pricing_fixture() {
    let source = include_str!("fixtures/pricing.brix");
    let module = parse(source).expect("pricing.brix should parse successfully");

    assert_eq!(module.items.len(), 5);

    // 1. config Item = { name: Str, base: Int }
    match &module.items[0] {
        Item::Config(ConfigDecl { name, body, .. }) => {
            assert_eq!(name, "Item");
            match body {
                ConfigBody::Record(fields) => {
                    assert_eq!(fields.len(), 2);
                    assert_eq!(fields[0].name, "name");
                    assert_eq!(fields[0].ty, Ty::Named("Str".into()));
                    assert_eq!(fields[1].name, "base");
                    assert_eq!(fields[1].ty, Ty::Named("Int".into()));
                }
                _ => panic!("Expected Record body for Item"),
            }
        }
        _ => panic!("Expected Item::Config for pricing.brix item 0"),
    }

    // 2. regime pricing { gen taxed(m: Int) = m * 1.2 }
    match &module.items[1] {
        Item::Regime(RegimeDecl { name, gens }) => {
            assert_eq!(name, "pricing");
            assert_eq!(gens.len(), 1);
            let gen_taxed = &gens[0];
            assert_eq!(gen_taxed.name, "taxed");
            assert_eq!(gen_taxed.params.len(), 1);
            assert_eq!(gen_taxed.params[0].name, "m");
            assert_eq!(gen_taxed.params[0].ty, Some(Ty::Named("Int".into())));
            assert_eq!(gen_taxed.ret, None);
            assert_eq!(
                gen_taxed.body,
                Expr::Bin {
                    op: BinOp::Mul,
                    lhs: Box::new(Expr::Var("m".into())),
                    rhs: Box::new(Expr::Num("1.2".into())),
                }
            );
        }
        _ => panic!("Expected Item::Regime for pricing.brix item 1"),
    }

    // 3. rule cost(i: Item) = taxed(i.base)
    match &module.items[2] {
        Item::Rule(Callable {
            name,
            params,
            ret,
            body,
        }) => {
            assert_eq!(name, "cost");
            assert_eq!(params.len(), 1);
            assert_eq!(params[0].name, "i");
            assert_eq!(params[0].ty, Some(Ty::Named("Item".into())));
            assert_eq!(*ret, None);
            assert_eq!(
                *body,
                Expr::Call {
                    func: "taxed".into(),
                    args: vec![Expr::Field(Box::new(Expr::Var("i".into())), "base".into())],
                }
            );
        }
        _ => panic!("Expected Item::Rule for pricing.brix item 2"),
    }

    // 4. let widget = Item { name: "widget", base: 10 }
    match &module.items[3] {
        Item::Let(LetDecl { name, ty, value }) => {
            assert_eq!(name, "widget");
            assert_eq!(*ty, None);
            assert_eq!(
                *value,
                Expr::Record {
                    config: "Item".into(),
                    fields: vec![
                        ("name".into(), Expr::Str("widget".into())),
                        ("base".into(), Expr::Num("10".into())),
                    ],
                }
            );
        }
        _ => panic!("Expected Item::Let for pricing.brix item 3"),
    }

    // 5. show cost(widget)
    match &module.items[4] {
        Item::Show(expr) => {
            assert_eq!(
                *expr,
                Expr::Call {
                    func: "cost".into(),
                    args: vec![Expr::Var("widget".into())],
                }
            );
        }
        _ => panic!("Expected Item::Show for pricing.brix item 4"),
    }
}

#[test]
fn test_parse_nat_fixture() {
    let source = include_str!("fixtures/nat.brix");
    let module = parse(source).expect("nat.brix should parse successfully");

    assert_eq!(module.items.len(), 3);

    // 1. config Nat = Zero | Succ(Nat)
    match &module.items[0] {
        Item::Config(ConfigDecl { name, body, .. }) => {
            assert_eq!(name, "Nat");
            match body {
                ConfigBody::Sum(variants) => {
                    assert_eq!(variants.len(), 2);
                    assert_eq!(variants[0].name, "Zero");
                    assert!(variants[0].params.is_empty());
                    assert_eq!(variants[1].name, "Succ");
                    assert_eq!(variants[1].params, vec![Ty::Named("Nat".into())]);
                }
                _ => panic!("Expected Sum body for Nat"),
            }
        }
        _ => panic!("Expected Item::Config for nat.brix item 0"),
    }

    // 2. fn double(n: Nat): Nat = match n { ... }
    match &module.items[1] {
        Item::Fn(Callable {
            name,
            params,
            ret,
            body,
        }) => {
            assert_eq!(name, "double");
            assert_eq!(params.len(), 1);
            assert_eq!(params[0].name, "n");
            assert_eq!(params[0].ty, Some(Ty::Named("Nat".into())));
            assert_eq!(*ret, Some(Ty::Named("Nat".into())));
            match body {
                Expr::Match {
                    scrutinee,
                    arms,
                    proving_exhaustive,
                } => {
                    assert_eq!(**scrutinee, Expr::Var("n".into()));
                    assert_eq!(arms.len(), 2);
                    assert!(!proving_exhaustive);

                    // Zero => Zero
                    assert_eq!(
                        arms[0].pattern,
                        Pattern::Ctor {
                            name: "Zero".into(),
                            args: vec![],
                        }
                    );
                    assert_eq!(arms[0].body, Expr::Var("Zero".into()));

                    // Succ(k) => Succ(Succ(double(k)))
                    assert_eq!(
                        arms[1].pattern,
                        Pattern::Ctor {
                            name: "Succ".into(),
                            args: vec![Pattern::Var("k".into())],
                        }
                    );
                    assert_eq!(
                        arms[1].body,
                        Expr::Call {
                            func: "Succ".into(),
                            args: vec![Expr::Call {
                                func: "Succ".into(),
                                args: vec![Expr::Call {
                                    func: "double".into(),
                                    args: vec![Expr::Var("k".into())],
                                }],
                            }],
                        }
                    );
                }
                _ => panic!("Expected Match body for double"),
            }
        }
        _ => panic!("Expected Item::Fn for nat.brix item 1"),
    }

    // 3. show double(Succ(Succ(Zero)))
    match &module.items[2] {
        Item::Show(expr) => {
            assert_eq!(
                *expr,
                Expr::Call {
                    func: "double".into(),
                    args: vec![Expr::Call {
                        func: "Succ".into(),
                        args: vec![Expr::Call {
                            func: "Succ".into(),
                            args: vec![Expr::Var("Zero".into())],
                        }],
                    }],
                }
            );
        }
        _ => panic!("Expected Item::Show for nat.brix item 2"),
    }
}

#[test]
fn test_parse_power_fixture() {
    let source = include_str!("fixtures/power.brix");
    let module = parse(source).expect("power.brix should parse successfully");

    assert_eq!(module.items.len(), 10);

    // Items 0..4 match pricing.brix

    // Item 4: let quote: Int @Audited = cost(widget)
    match &module.items[4] {
        Item::Let(LetDecl { name, ty, value }) => {
            assert_eq!(name, "quote");
            assert_eq!(
                *ty,
                Some(Ty::Graded(
                    Box::new(Ty::Named("Int".into())),
                    Grade::Audited
                ))
            );
            assert_eq!(
                *value,
                Expr::Call {
                    func: "cost".into(),
                    args: vec![Expr::Var("widget".into())],
                }
            );
        }
        _ => panic!("Expected Item::Let for power.brix item 4"),
    }

    // Item 5: let ok = prove within_budget(quote)
    match &module.items[5] {
        Item::Let(LetDecl { name, ty, value }) => {
            assert_eq!(name, "ok");
            assert_eq!(*ty, None);
            assert_eq!(
                *value,
                Expr::Prove(Box::new(Expr::Call {
                    func: "within_budget".into(),
                    args: vec![Expr::Var("quote".into())],
                }))
            );
        }
        _ => panic!("Expected Item::Let for power.brix item 5"),
    }

    // Item 6: witness w = why(ok)
    match &module.items[6] {
        Item::Witness { name, value } => {
            assert_eq!(name, "w");
            assert_eq!(*value, Expr::Why(Box::new(Expr::Var("ok".into()))));
        }
        _ => panic!("Expected Item::Witness for power.brix item 6"),
    }

    // Item 7: witness w2 = why(quote)
    match &module.items[7] {
        Item::Witness { name, value } => {
            assert_eq!(name, "w2");
            assert_eq!(*value, Expr::Why(Box::new(Expr::Var("quote".into()))));
        }
        _ => panic!("Expected Item::Witness for power.brix item 7"),
    }

    // Item 8: show w then w2
    match &module.items[8] {
        Item::Show(expr) => {
            assert_eq!(
                *expr,
                Expr::Bin {
                    op: BinOp::Then,
                    lhs: Box::new(Expr::Var("w".into())),
                    rhs: Box::new(Expr::Var("w2".into())),
                }
            );
        }
        _ => panic!("Expected Item::Show for power.brix item 8"),
    }

    // Item 9: show audit cost(widget)
    match &module.items[9] {
        Item::Show(expr) => {
            assert_eq!(
                *expr,
                Expr::Audit(Box::new(Expr::Call {
                    func: "cost".into(),
                    args: vec![Expr::Var("widget".into())],
                }))
            );
        }
        _ => panic!("Expected Item::Show for power.brix item 9"),
    }
}

#[test]
fn test_negative_parse_inputs() {
    let invalid_inputs = [
        "let x = ",
        "config =",
        "regime r { gen f( = 1 }",
        "show match x { Zero }",
        "let a: Money @InvalidGrade = 10",
        "witness = 10",
        "show \"unterminated string",
        // `proving` not followed by `exhaustive` is a clear parse error.
        "show match x { Zero => 1 } proving foo",
    ];

    for input in &invalid_inputs {
        let res = parse(input);
        assert!(
            res.is_err(),
            "Expected parse error for malformed input: '{}'",
            input
        );
    }
}

#[test]
fn test_match_proving_exhaustive_true() {
    let source = "show match x { A => 1  B => 2 } proving exhaustive";
    let module = parse(source).expect("match ... proving exhaustive should parse");
    assert_eq!(module.items.len(), 1);
    match &module.items[0] {
        Item::Show(Expr::Match {
            arms,
            proving_exhaustive,
            ..
        }) => {
            assert_eq!(arms.len(), 2);
            assert!(*proving_exhaustive);
        }
        other => panic!("Expected Item::Show(Expr::Match), got {:?}", other),
    }
}

#[test]
fn test_match_without_proving_exhaustive_suffix() {
    let source = "show match x { A => 1  B => 2 }";
    let module = parse(source).expect("plain match should still parse");
    assert_eq!(module.items.len(), 1);
    match &module.items[0] {
        Item::Show(Expr::Match {
            arms,
            proving_exhaustive,
            ..
        }) => {
            assert_eq!(arms.len(), 2);
            assert!(!*proving_exhaustive);
        }
        other => panic!("Expected Item::Show(Expr::Match), got {:?}", other),
    }
}

#[test]
fn test_proving_identifier_regression() {
    // `proving` is only contextual immediately after a match's closing '}';
    // as an ordinary identifier elsewhere it must keep working.
    let source = "let proving = 1\nshow proving";
    let module = parse(source).expect("'proving' should parse as an ordinary identifier");
    assert_eq!(module.items.len(), 2);
    match &module.items[0] {
        Item::Let(LetDecl { name, value, .. }) => {
            assert_eq!(name, "proving");
            assert_eq!(*value, Expr::Num("1".into()));
        }
        other => panic!("Expected Item::Let, got {:?}", other),
    }
    match &module.items[1] {
        Item::Show(expr) => assert_eq!(*expr, Expr::Var("proving".into())),
        other => panic!("Expected Item::Show, got {:?}", other),
    }
}

#[test]
fn named_field_variant_desugars_to_one_record_parameter() {
    // A named-field variant and a positional one carrying the same anonymous
    // record are the same declaration; only the spelling differs.
    let source = "config C = M { frame: Int, atk: Int } | N";
    let module = parse(source).expect("named-field variants should parse");

    let Item::Config(ConfigDecl {
        body: ConfigBody::Sum(variants),
        ..
    }) = &module.items[0]
    else {
        panic!("expected a sum config, got {:?}", module.items[0]);
    };
    assert_eq!(variants.len(), 2);

    assert_eq!(variants[0].name, "M");
    match variants[0].params.as_slice() {
        [Ty::Record(fields)] => {
            let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
            assert_eq!(names, ["frame", "atk"], "declaration order is preserved");
        }
        other => panic!("expected one record parameter, got {other:?}"),
    }

    // A nullary variant is unaffected.
    assert_eq!(variants[1].name, "N");
    assert!(variants[1].params.is_empty());
}

#[test]
fn positional_variants_are_unchanged() {
    let source = "config C = M(Int, Str) | N";
    let module = parse(source).expect("positional variants should still parse");
    let Item::Config(ConfigDecl {
        body: ConfigBody::Sum(variants),
        ..
    }) = &module.items[0]
    else {
        panic!("expected a sum config");
    };
    assert_eq!(
        variants[0].params,
        vec![Ty::Named("Int".into()), Ty::Named("Str".into())]
    );
    assert!(variants[1].params.is_empty());
}

#[test]
fn comparison_binds_tighter_than_composition_and_looser_than_arithmetic() {
    // `a + 1 > b * 2` must group as `(a + 1) > (b * 2)`.
    let module = parse("let x = a + 1 > b * 2").expect("parse");
    let Item::Let(LetDecl { value, .. }) = &module.items[0] else {
        panic!("expected a let");
    };
    match value {
        Expr::Bin { op, lhs, rhs } => {
            assert_eq!(*op, BinOp::Gt);
            assert!(
                matches!(**lhs, Expr::Bin { op: BinOp::Add, .. }),
                "lhs should be the addition, got {lhs:?}"
            );
            assert!(
                matches!(**rhs, Expr::Bin { op: BinOp::Mul, .. }),
                "rhs should be the multiplication, got {rhs:?}"
            );
        }
        other => panic!("expected a comparison at the root, got {other:?}"),
    }
}

#[test]
fn comparison_operators_do_not_chain() {
    // Refused by name rather than read as `(a < b) < c`. Refusing keeps the
    // door open to defining chained comparison later without breaking any
    // program that exists today.
    let err = parse("let x = 1 < 2 < 3").expect_err("chained comparison must be refused");
    assert!(
        err.to_string().contains("do not chain"),
        "expected a chaining diagnostic, got: {err}"
    );
}

#[test]
fn boolean_literals_parse() {
    for (src, expected) in [("let t = true", true), ("let f = false", false)] {
        let module = parse(src).expect("parse");
        let Item::Let(LetDecl { value, .. }) = &module.items[0] else {
            panic!("expected a let");
        };
        assert_eq!(*value, Expr::Bool(expected));
    }
}

#[test]
fn grade_names_are_contextual_not_reserved() {
    // `Derived`/`Audited`/`Proven` name a grade only after `@`. Reserving them
    // stopped SOC's own outcome lattice from being spelled in Brix, which is
    // an odd thing for a language to forbid about its own vocabulary.
    let module = parse("config Outcome = Unknown | Derived | Audited | Proven")
        .expect("grade names must be usable as ordinary variant names");
    let Item::Config(ConfigDecl {
        body: ConfigBody::Sum(variants),
        ..
    }) = &module.items[0]
    else {
        panic!("expected a sum config");
    };
    let names: Vec<&str> = variants.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["Unknown", "Derived", "Audited", "Proven"]);

    // And they still mean a grade in grade position.
    let module = parse("let x: Int @Proven = 1").expect("grade annotations still parse");
    let Item::Let(LetDecl { ty, .. }) = &module.items[0] else {
        panic!("expected a let");
    };
    assert_eq!(
        *ty,
        Some(Ty::Graded(Box::new(Ty::Named("Int".into())), Grade::Proven))
    );
}

#[test]
fn boolean_operators_precedence_and_associativity() {
    // `!a && b > 3 || c == true` must parse as `((!a) && (b > 3)) || (c == true)`
    let module = parse("let x = !a && b > 3 || c == true").expect("parse boolean expression");
    let Item::Let(LetDecl { value, .. }) = &module.items[0] else {
        panic!("expected a let");
    };
    let Expr::Bin { op, lhs, rhs } = value else {
        panic!("expected binary op at root");
    };
    assert_eq!(*op, BinOp::OrOr);

    // LHS of || is `!a && b > 3`
    let Expr::Bin {
        op: lhs_op,
        lhs: and_lhs,
        rhs: and_rhs,
    } = &**lhs
    else {
        panic!("expected && on lhs of ||");
    };
    assert_eq!(*lhs_op, BinOp::AndAnd);
    assert!(matches!(&**and_lhs, Expr::Not(inner) if matches!(&**inner, Expr::Var(v) if v == "a")));
    assert!(matches!(&**and_rhs, Expr::Bin { op: BinOp::Gt, .. }));

    // RHS of || is `c == true`
    assert!(matches!(&**rhs, Expr::Bin { op: BinOp::Eq, .. }));

    // Left associativity of &&: `a && b && c` -> `(a && b) && c`
    let module = parse("let x = a && b && c").expect("parse left-assoc &&");
    let Item::Let(LetDecl { value, .. }) = &module.items[0] else {
        panic!("expected a let");
    };
    let Expr::Bin { op, lhs, rhs } = value else {
        panic!("expected binary op");
    };
    assert_eq!(*op, BinOp::AndAnd);
    assert!(matches!(&**rhs, Expr::Var(v) if v == "c"));
    assert!(matches!(
        &**lhs,
        Expr::Bin {
            op: BinOp::AndAnd,
            ..
        }
    ));

    // Left associativity of ||: `a || b || c` -> `(a || b) || c`
    let module = parse("let x = a || b || c").expect("parse left-assoc ||");
    let Item::Let(LetDecl { value, .. }) = &module.items[0] else {
        panic!("expected a let");
    };
    let Expr::Bin { op, lhs, rhs } = value else {
        panic!("expected binary op");
    };
    assert_eq!(*op, BinOp::OrOr);
    assert!(matches!(&**rhs, Expr::Var(v) if v == "c"));
    assert!(matches!(
        &**lhs,
        Expr::Bin {
            op: BinOp::OrOr,
            ..
        }
    ));
}

#[test]
fn single_ampersand_is_rejected_with_suggestion() {
    let err = parse("let x = a & b").expect_err("single & must be rejected");
    assert!(err.to_string().contains("did you mean '&&' ?"));
}

#[test]
fn unary_minus_precedence_and_associativity() {
    // `-a * b` must parse as `(-a) * b`: unary minus binds tighter than every
    // binary operator, but a numeral would fold directly into a literal, so
    // use a variable operand to force the general `0 - e` desugaring.
    let module = parse("let x = -a * b").expect("parse unary minus with *");
    let Item::Let(LetDecl { value, .. }) = &module.items[0] else {
        panic!("expected a let");
    };
    let Expr::Bin { op, lhs, rhs } = value else {
        panic!("expected binary op at root");
    };
    assert_eq!(*op, BinOp::Mul);
    assert!(matches!(&**rhs, Expr::Var(v) if v == "b"));
    // The negated operand desugars to `0 - a`.
    let Expr::Bin {
        op: neg_op,
        lhs: neg_lhs,
        rhs: neg_rhs,
    } = &**lhs
    else {
        panic!("expected unary minus to desugar to a Sub node");
    };
    assert_eq!(*neg_op, BinOp::Sub);
    assert_eq!(**neg_lhs, Expr::Num("0".into()));
    assert!(matches!(&**neg_rhs, Expr::Var(v) if v == "a"));

    // `-a + b` must parse as `(-a) + b`.
    let module = parse("let x = -a + b").expect("parse unary minus with +");
    let Item::Let(LetDecl { value, .. }) = &module.items[0] else {
        panic!("expected a let");
    };
    let Expr::Bin { op, lhs, rhs } = value else {
        panic!("expected binary op at root");
    };
    assert_eq!(*op, BinOp::Add);
    assert!(matches!(&**rhs, Expr::Var(v) if v == "b"));
    assert!(matches!(&**lhs, Expr::Bin { op: BinOp::Sub, .. }));
}

#[test]
fn unary_minus_binds_tighter_than_postfix_field_access() {
    // `-a.field` must parse as `-(a.field)`: postfix `.field` binds tighter
    // than the prefix `-`, so the field projection is the operand being
    // negated, not the other way around.
    let module = parse("let x = -a.field").expect("parse -a.field");
    let Item::Let(LetDecl { value, .. }) = &module.items[0] else {
        panic!("expected a let");
    };
    let Expr::Bin {
        op,
        lhs: zero,
        rhs: operand,
    } = value
    else {
        panic!("expected unary minus to desugar to a Sub node");
    };
    assert_eq!(*op, BinOp::Sub);
    assert_eq!(**zero, Expr::Num("0".into()));
    match &**operand {
        Expr::Field(base, field) => {
            assert!(matches!(&**base, Expr::Var(v) if v == "a"));
            assert_eq!(field, "field");
        }
        other => panic!("expected -(a.field), got {other:?}"),
    }
}

#[test]
fn unary_minus_on_a_numeral_folds_to_a_negative_literal() {
    // `-7` must be `Expr::Num("-7")`, not `Sub(0, 7)` — this is what makes
    // `i64::MIN` (`-9223372036854775808`) representable at all: its positive
    // magnitude alone overflows `i64::MAX` and cannot be built via `0 - n`.
    let module = parse("let x = -7").expect("parse -7");
    let Item::Let(LetDecl { value, .. }) = &module.items[0] else {
        panic!("expected a let");
    };
    assert_eq!(*value, Expr::Num("-7".into()));

    let module = parse("let x = -9223372036854775808").expect("parse i64::MIN literal");
    let Item::Let(LetDecl { value, .. }) = &module.items[0] else {
        panic!("expected a let");
    };
    assert_eq!(*value, Expr::Num("-9223372036854775808".into()));

    // Double negation does NOT fold across the intervening Sub: `--7` is
    // `0 - (-7)`, one recursive `0 - e` desugaring wrapping a folded literal.
    let module = parse("let x = --7").expect("parse --7");
    let Item::Let(LetDecl { value, .. }) = &module.items[0] else {
        panic!("expected a let");
    };
    let Expr::Bin { op, lhs, rhs } = value else {
        panic!("expected binary op at root");
    };
    assert_eq!(*op, BinOp::Sub);
    assert_eq!(**lhs, Expr::Num("0".into()));
    assert_eq!(**rhs, Expr::Num("-7".into()));
}

#[test]
fn unary_minus_on_parenthesised_i64_min_still_faults_to_desugaring() {
    // `-(-9223372036854775808)` cannot fold (the operand is parenthesised,
    // not a bare numeral immediately after `-`), so it desugars to
    // `0 - (-9223372036854775808)` — a value only the *evaluator's* checked
    // arithmetic can refuse (see l3_v2_stage_b.rs for the fault test).
    let module = parse("let x = -(-9223372036854775808)").expect("parse -(i64::MIN)");
    let Item::Let(LetDecl { value, .. }) = &module.items[0] else {
        panic!("expected a let");
    };
    let Expr::Bin { op, lhs, rhs } = value else {
        panic!("expected binary op at root");
    };
    assert_eq!(*op, BinOp::Sub);
    assert_eq!(**lhs, Expr::Num("0".into()));
    assert_eq!(**rhs, Expr::Num("-9223372036854775808".into()));
}
