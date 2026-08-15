use brix_lower::{check_module, LowerError};
use brix_semantic::Outcome;
use brix_syntax::parse;
use soc_regimes::type_realization::{bool_ty, Ty as TrTy, TypeError};

#[test]
fn test_id_fixture_proven() {
    let source = include_str!("fixtures/id.brix");
    let module = parse(source).expect("id.brix should parse");
    let results = check_module(&module);

    assert_eq!(results.len(), 1);
    let check_res = results
        .into_iter()
        .next()
        .unwrap()
        .expect("let r binding should lower & type-check successfully");

    assert_eq!(check_res.name, "r");
    // The λ-calculus core (var/λ/app) is discharged, so `id(42)` earns Proven.
    assert_eq!(check_res.outcome, Outcome::Proven);
    assert_eq!(check_res.ty, Some(TrTy::Con("Int")));
}

#[test]
fn test_let_lit_earns_proven() {
    let source = "let x = 42";
    let module = parse(source).expect("let x = 42 should parse");
    let results = check_module(&module);

    assert_eq!(results.len(), 1);
    let check_res = results
        .into_iter()
        .next()
        .unwrap()
        .expect("let x = 42 should lower & prove successfully");

    assert_eq!(check_res.name, "x");
    // A pure literal rests only on the discharged (tight) `g_lit`, so it earns Proven.
    assert_eq!(check_res.outcome, Outcome::Proven);
    assert_eq!(check_res.ty, Some(TrTy::Con("Int")));
}

#[test]
fn test_unsupported_construct_negative() {
    // Witness composition (`then`/`and`) is still outside the current fragment.
    let source = "let y = 1 then 2";
    let module = parse(source).expect("witness-composition expression should parse");
    let results = check_module(&module);

    assert_eq!(results.len(), 1);
    let (name, err) = results
        .into_iter()
        .next()
        .unwrap()
        .expect_err("unsupported construct should fail lowering");

    assert_eq!(name, "y");
    match err {
        LowerError::Unsupported(msg) => {
            assert!(msg.contains("L2-first fragment"));
        }
        _ => panic!("Expected LowerError::Unsupported, got {:?}", err),
    }
}

#[test]
fn proving_exhaustive_match_gets_kernel_certified_coverage() {
    use brix_lower::CoverageOutcome;
    let src = "config Opt = None | Some(Int)\nlet a = match Some(3) {\n  None => 0\n  Some(k) => k\n} proving exhaustive\n";
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    let cr = results[0].as_ref().expect("should check");
    assert_eq!(cr.name, "a");
    // The typing result is Proven; coverage is a separate, kernel-Proven claim.
    assert_eq!(cr.outcome, Outcome::Proven);
    assert_eq!(cr.coverage, Some(CoverageOutcome::Proven));
}

#[test]
fn proving_exhaustive_with_wildcard_is_not_certified_but_still_checks() {
    use brix_lower::CoverageOutcome;
    // A wildcard is structurally exhaustive (ordinary match is fine) but is
    // outside the certified fragment → coverage Unknown, never a false Proven.
    let src = "config Opt = None | Some(Int)\nlet a = match Some(3) {\n  Some(k) => k\n  _ => 0\n} proving exhaustive\n";
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    let cr = results[0].as_ref().expect("should check");
    // The catch-all is structurally exhaustive, but its repeated coproduct
    // premises are not represented in the realization tree yet. Its type and
    // distinct coverage certificate therefore both remain below Proven.
    assert_eq!(cr.outcome, Outcome::Audited);
    assert!(matches!(cr.coverage, Some(CoverageOutcome::Unknown(_))));
}

#[test]
fn ordinary_match_has_no_coverage_claim() {
    let src =
        "config Opt = None | Some(Int)\nlet a = match Some(3) {\n  None => 0\n  Some(k) => k\n}\n";
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    let cr = results[0].as_ref().expect("should check");
    assert_eq!(cr.outcome, Outcome::Proven);
    assert_eq!(cr.coverage, None);
}

#[test]
fn grade_assertion_satisfied_and_downgrade_ok() {
    // @Proven on a discharged literal is satisfied; @Derived on an Audited
    // value is a free downgrade (still checks; the earned grade is reported).
    for (src, name, grade) in [
        ("let a: Int @Proven = 42", "a", Outcome::Proven),
        ("let b: Int @Audited = 1 + 2", "b", Outcome::Audited),
        ("let c: Int @Derived = 1 + 2", "c", Outcome::Audited),
    ] {
        let module = parse(src).expect("parse");
        let results = check_module(&module);
        let cr = results[0]
            .as_ref()
            .unwrap_or_else(|(n, e)| panic!("{src}: {n}: {e:?}"));
        assert_eq!(cr.name, name, "{src}");
        assert_eq!(cr.outcome, grade, "{src}");
    }
}

#[test]
fn grade_over_claim_is_epistemic_erasure() {
    // Asserting @Proven on an Audited (arithmetic) value over-claims → erasure.
    let module = parse("let d: Int @Proven = 1 + 2").expect("parse");
    let (name, err) = check_module(&module)
        .into_iter()
        .next()
        .unwrap()
        .expect_err("over-claiming @Proven must fail");
    assert_eq!(name, "d");
    assert_eq!(
        err,
        LowerError::GradeErasure {
            asserted: "Proven".to_string(),
            actual: "Audited".to_string(),
        }
    );
}

#[test]
fn test_unresolved_function_call() {
    let source = "let z = unknown(42)";
    let module = parse(source).expect("call expression should parse");
    let results = check_module(&module);

    assert_eq!(results.len(), 1);
    let (name, err) = results
        .into_iter()
        .next()
        .unwrap()
        .expect_err("unresolved call should fail lowering");

    assert_eq!(name, "z");
    assert_eq!(err, LowerError::Unresolved("unknown".to_string()));
}

#[test]
fn test_record_and_field_proven() {
    let source = r#"
        let p = Item { x: 1, y: 2 }
        let a = p.x
    "#;
    let module = parse(source).expect("record program should parse");
    let results = check_module(&module);

    assert_eq!(results.len(), 2);

    let res_p = results[0]
        .as_ref()
        .expect("let p should lower & type-check successfully");
    assert_eq!(res_p.name, "p");
    assert_eq!(res_p.outcome, Outcome::Proven);
    assert_eq!(
        res_p.ty,
        Some(TrTy::Record(vec![
            ("x".to_string(), TrTy::Con("Int")),
            ("y".to_string(), TrTy::Con("Int")),
        ]))
    );

    let res_a = results[1]
        .as_ref()
        .expect("let a should lower & type-check successfully");
    assert_eq!(res_a.name, "a");
    assert_eq!(res_a.outcome, Outcome::Proven);
    assert_eq!(res_a.ty, Some(TrTy::Con("Int")));
}

#[test]
fn arithmetic_int_float_and_mixed_promotion_typecheck_audited() {
    // Int+Int→Int, Float+Float→Float, and mixed Int+Float→Float (via the
    // Int↪Float promotion witness) all type-check (Audited — g_arith is not
    // discharged) with the expected type.
    let cases = [
        ("let a = 1 + 2", "a", TrTy::Con("Int")),
        ("let b = 1.5 + 2.5", "b", TrTy::Con("Float")),
        ("let c = 1 + 2.5", "c", TrTy::Con("Float")),
        ("let d = 3.0 * 2", "d", TrTy::Con("Float")),
        ("let e = 10 - 4 * 2", "e", TrTy::Con("Int")),
        // Div forces the field of fractions: Int/Int → Float.
        ("let q = 7 / 2", "q", TrTy::Con("Float")),
        ("let h = 9.0 / 4.0", "h", TrTy::Con("Float")),
    ];
    for (src, name, want) in cases {
        let module = parse(src).unwrap_or_else(|e| panic!("{src}: parse {e:?}"));
        let results = check_module(&module);
        assert_eq!(results.len(), 1, "{src}");
        let cr = results[0]
            .as_ref()
            .unwrap_or_else(|(n, e)| panic!("{src}: {n}: {e:?}"));
        assert_eq!(cr.name, name, "{src}");
        assert_eq!(cr.outcome, Outcome::Audited, "{src} wrong grade");
        assert_eq!(cr.ty, Some(want), "{src} wrong type");
    }
}

#[test]
fn function_using_arithmetic_reaches_audited() {
    // `n + n` defaults the parameter to Int, so `double : Int → Int` and the
    // applied result types as Int (Audited — g_arith not discharged).
    let src = "fn double(n) = n + n\nlet r = double(21)";
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    assert_eq!(results.len(), 1);
    let cr = results[0].as_ref().expect("double(21) should type-check");
    assert_eq!(cr.name, "r");
    assert_eq!(cr.outcome, Outcome::Audited);
    assert_eq!(cr.ty, Some(TrTy::Con("Int")));
}

#[test]
fn arithmetic_on_string_is_a_type_error() {
    // "hi" + 1 mixes a non-numeric operand → a real type error, not Proven.
    let module = parse("let bad = 1 + \"hi\"").expect("parse");
    let results = check_module(&module);
    assert_eq!(results.len(), 1);
    let (name, err) = results
        .into_iter()
        .next()
        .unwrap()
        .expect_err("string arithmetic should fail type checking");
    assert_eq!(name, "bad");
    assert!(
        matches!(err, LowerError::TypeError(_)),
        "expected a TypeError, got {err:?}"
    );
}

#[test]
fn str_literal_record_and_field_earn_proven() {
    // String literals, nonempty records, and field access all rest only on
    // discharged introduction/elimination rules.
    let src = "let s = \"hi\"\nlet w = Item { name: \"widget\", base: 10 }\nlet n = w.name\n";
    let module = brix_syntax::parse(src).expect("parse");
    let results = brix_lower::check_module(&module);
    assert_eq!(results.len(), 3);

    let expected = [
        ("s", Outcome::Proven),
        ("w", Outcome::Proven),
        ("n", Outcome::Proven),
    ];
    for (r, (name, grade)) in results.iter().zip(expected) {
        let cr = r
            .as_ref()
            .unwrap_or_else(|(name, e)| panic!("{name}: {e:?}"));
        assert_eq!(cr.name, name);
        assert_eq!(cr.outcome, grade, "{name} has wrong grade");
    }
}

#[test]
fn test_declared_record_config_success() {
    let src = r#"
        config Item = { name: Str, base: Int }
        let w = Item { name: "widget", base: 10 }
    "#;
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    assert_eq!(results.len(), 1);
    let check_res = results[0]
        .as_ref()
        .expect("declared record literal should lower & type-check");
    assert_eq!(check_res.name, "w");
    assert_eq!(check_res.outcome, Outcome::Proven);
}

#[test]
fn test_declared_record_config_missing_field() {
    let src = r#"
        config Item = { name: Str, base: Int }
        let w = Item { name: "widget" }
    "#;
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    assert_eq!(results.len(), 1);
    let (name, err) = results[0]
        .as_ref()
        .expect_err("missing field should fail lowering");
    assert_eq!(name, "w");
    assert_eq!(
        *err,
        LowerError::MissingField {
            config: "Item".to_string(),
            field: "base".to_string(),
        }
    );
}

#[test]
fn test_declared_record_config_unknown_field() {
    let src = r#"
        config Item = { name: Str, base: Int }
        let w = Item { name: "widget", base: 10, extra: 1 }
    "#;
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    assert_eq!(results.len(), 1);
    let (name, err) = results[0]
        .as_ref()
        .expect_err("unknown field should fail lowering");
    assert_eq!(name, "w");
    assert_eq!(
        *err,
        LowerError::UnknownField {
            config: "Item".to_string(),
            field: "extra".to_string(),
        }
    );
}

#[test]
fn test_sum_config_used_as_record_literal_error() {
    let src = r#"
        config Item = Zero | Succ(Nat)
        let w = Item { name: "widget" }
    "#;
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    assert_eq!(results.len(), 1);
    let (name, err) = results[0]
        .as_ref()
        .expect_err("sum config used as record literal should fail lowering");
    assert_eq!(name, "w");
    match err {
        LowerError::Unsupported(msg) => {
            assert!(
                msg.contains("sum config"),
                "expected 'sum config' in message, got: {msg}"
            );
        }
        _ => panic!("Expected LowerError::Unsupported, got {:?}", err),
    }
}

#[test]
fn test_sum_match_honest_outcomes() {
    let src = r#"
        config Opt = None | Some(Int)
        let a = match Some(3) { None => 0 Some(k) => k }
        let b = match None { None => 0 Some(k) => k }
    "#;
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    assert_eq!(results.len(), 2);

    let res_a = results[0]
        .as_ref()
        .expect("let a should lower & type-check");
    assert_eq!(res_a.name, "a");
    assert_eq!(res_a.outcome, Outcome::Proven);
    assert_eq!(res_a.ty, Some(TrTy::Con("Int")));

    let res_b = results[1]
        .as_ref()
        .expect("let b should lower & type-check");
    assert_eq!(res_b.name, "b");
    // `None` is a nullary constructor. This used to stay Audited on the
    // grounds that "the current kernel has no zero/unit introduction" — true,
    // but the wrong test: that is what the *structural* coproduct rules must
    // correspond to, and a nullary constructor has no payload to inject, so it
    // is a zero-premise introduction discharged on `g_lit`'s ground instead.
    // See `g_ctor_nullary`'s doc and
    // `soc_regimes::type_realization::tests::zero_arity_intro_generators_are_faithful`.
    //
    // Every leaf here is independently discharged — g_match_split,
    // g_ctor_nullary, g_lit, g_var, g_match — and the match is exhaustive, so
    // it emits `g_match` rather than the undischarged `g_match_catchall`.
    assert_eq!(res_b.outcome, Outcome::Proven);
    assert_eq!(res_b.ty, Some(TrTy::Con("Int")));
}

#[test]
fn test_sum_match_non_exhaustive() {
    let src = r#"
        config Opt = None | Some(Int)
        let c = match Some(3) { Some(k) => k }
    "#;
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    assert_eq!(results.len(), 1);

    let (name, err) = results[0]
        .as_ref()
        .expect_err("non-exhaustive match should fail type check");
    assert_eq!(name, "c");
    assert_eq!(
        *err,
        LowerError::TypeError(TypeError::NonExhaustive(vec!["None".to_string()]))
    );
}

/// A recursive `config` is still refused — but by name, against the
/// declaration.
///
/// This previously asserted `Unresolved("Succ")`, which was the *symptom* of
/// the sum being silently dropped from the constructor table: an error naming
/// a correct use rather than the unsupported declaration. The refusal itself
/// is unchanged — recursion is still not supported, and nothing that was
/// rejected before is accepted now.
#[test]
fn test_recursive_sum_refused_by_name() {
    let src = r#"
        config Nat = Zero | Succ(Nat)
        let x = Succ(1)
    "#;
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    assert_eq!(results.len(), 1);

    let (name, err) = results[0]
        .as_ref()
        .expect_err("a recursive config is not supported yet");
    assert_eq!(name, "x");
    assert_eq!(
        *err,
        LowerError::RecursiveConfig {
            config: "Nat".to_string(),
            cycle: vec!["Nat".to_string(), "Nat".to_string()],
        }
    );
}

/// The nullary constructor of a refused `config` reports the same fault.
///
/// Without this it falls through to the variable path and surfaces as
/// `Unbound("Zero")` — a second error about correct code, and a different one
/// from its sibling constructor, for the same single cause.
#[test]
fn test_nullary_ctor_of_refused_config_reports_the_declaration() {
    let src = r#"
        config Nat = Zero | Succ(Nat)
        let x = Zero
    "#;
    let module = parse(src).expect("parse");
    let results = check_module(&module);

    let (name, err) = results[0].as_ref().expect_err("Nat is still refused");
    assert_eq!(name, "x");
    assert!(
        matches!(err, LowerError::RecursiveConfig { config, .. } if config == "Nat"),
        "expected the declaration's fault, got {err:?}"
    );
}

/// Indirect recursion is caught at the point the cycle closes, and the chain
/// is reported rather than just the entry point.
#[test]
fn test_mutually_recursive_configs_report_the_cycle() {
    let src = r#"
        config A = MkA(B)
        config B = MkB(A)
        let x = MkA(1)
    "#;
    let module = parse(src).expect("parse");
    let results = check_module(&module);

    let (_, err) = results[0]
        .as_ref()
        .expect_err("mutual recursion is refused");
    match err {
        LowerError::RecursiveConfig { cycle, .. } => {
            assert!(
                cycle.len() >= 2 && cycle.first() == cycle.last(),
                "the cycle must close on itself, got {cycle:?}"
            );
        }
        other => panic!("expected RecursiveConfig, got {other:?}"),
    }
}

/// A sum variant may carry another `config` — the composition that makes a
/// data model out of data models.
#[test]
fn test_sum_variant_takes_another_config() {
    let src = r#"
        config Attribute = Dark | Earth
        config Stats = { atk: Int }
        config Card = MonsterCard(Attribute, Stats) | SpellCard(Int)

        let s = Stats { atk: 1500 }
        let m = MonsterCard(Earth, s)
        let p = SpellCard(3)
    "#;
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    assert_eq!(results.len(), 3);

    for (i, expected) in [("s", None), ("m", Some("Card")), ("p", Some("Card"))]
        .iter()
        .enumerate()
    {
        let r = results[i]
            .as_ref()
            .unwrap_or_else(|(n, e)| panic!("binding '{n}' should check, got {e:?}"));
        assert_eq!(&r.name, expected.0);
        if let Some(ty) = expected.1 {
            assert!(
                format!("{:?}", r.ty).contains(ty),
                "binding '{}' should have type {ty}, got {:?}",
                r.name,
                r.ty
            );
        }
    }
}

/// A variant parameter naming a type that does not exist is reported against
/// the declaration, naming both the variant and the offending type.
#[test]
fn test_unknown_variant_type_names_the_declaration() {
    let src = r#"
        config Card = MonsterCard(Nonexistent)
        let m = MonsterCard(1)
    "#;
    let module = parse(src).expect("parse");
    let results = check_module(&module);

    let (_, err) = results[0].as_ref().expect_err("Nonexistent is not a type");
    assert_eq!(
        *err,
        LowerError::UnknownVariantType {
            config: "Card".to_string(),
            variant: "MonsterCard".to_string(),
            ty: "Nonexistent".to_string(),
        }
    );
}

/// A named-field sum variant checks, and its constructor is applied with the
/// same named syntax it was declared with.
#[test]
fn test_named_field_variant_constructs_and_checks() {
    let src = r#"
        config Frame = Normal | Effect
        config Stats = { atk: Int, def: Int }
        config Card =
            MonsterCard { frame: Frame, stats: Stats }
          | SpellCard { subtype: Int }

        let m = MonsterCard { frame: Effect, stats: Stats { atk: 1500, def: 1400 } }
        let s = SpellCard { subtype: 2 }
    "#;
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    assert_eq!(results.len(), 2);

    for r in &results {
        let cr = r
            .as_ref()
            .unwrap_or_else(|(n, e)| panic!("binding '{n}' should check, got {e:?}"));
        assert!(
            format!("{:?}", cr.ty).contains("Card"),
            "'{}' should be a Card, got {:?}",
            cr.name,
            cr.ty
        );
    }
}

/// The field checks that apply to a record config apply to a named-field
/// variant too — the desugaring must not lose them.
#[test]
fn test_named_field_variant_field_errors() {
    for (src, expected) in [
        (
            "config C = M { a: Int, b: Int }\nlet x = M { a: 1 }",
            LowerError::MissingField {
                config: "M".to_string(),
                field: "b".to_string(),
            },
        ),
        (
            "config C = M { a: Int }\nlet x = M { a: 1, zz: 2 }",
            LowerError::UnknownField {
                config: "M".to_string(),
                field: "zz".to_string(),
            },
        ),
    ] {
        let module = parse(src).expect("parse");
        let results = check_module(&module);
        let (_, err) = results[0]
            .as_ref()
            .expect_err("field mismatch must be refused");
        assert_eq!(*err, expected, "for: {src}");
    }
}

/// A declared type is a contract, not decoration.
///
/// Every case below previously checked clean — most of them as `@Proven`,
/// which is the part that mattered: a program that looked like it declared a
/// contract collected the strongest grade with the contract unchecked.
#[test]
fn test_declared_types_are_checked() {
    // A `let` annotation must match the inferred type.
    let module = parse("let x: Str = 42").expect("parse");
    let results = check_module(&module);
    let (_, err) = results[0].as_ref().expect_err("Str vs Int must be refused");
    assert_eq!(
        *err,
        LowerError::TypeAnnotationMismatch {
            declared: "Str".to_string(),
            inferred: "Int".to_string(),
        }
    );

    // A declared type must name something real, in an annotation or in a
    // config declaration.
    for src in [
        "let y: Nonexistent = 42",
        "config Item = { base: Nonexistent }\nlet z = Item { base: 10 }",
    ] {
        let module = parse(src).expect("parse");
        let results = check_module(&module);
        let (_, err) = results[0]
            .as_ref()
            .expect_err("an undeclared type must be refused");
        assert_eq!(
            *err,
            LowerError::UnknownDeclaredType("Nonexistent".to_string()),
            "for: {src}"
        );
    }

    // A record literal's field values must match the declared field types.
    let module =
        parse("config Item = { name: Str, base: Int }\nlet bad = Item { name: 1, base: \"oops\" }")
            .expect("parse");
    let results = check_module(&module);
    let (_, err) = results[0]
        .as_ref()
        .expect_err("swapped field types must be refused");
    assert_eq!(
        *err,
        LowerError::RecordFieldTypeMismatch {
            config: "Item".to_string(),
            field: "name".to_string(),
            declared: "Str".to_string(),
            inferred: "Int".to_string(),
        }
    );
}

/// Correct programs still check, including where a declared type is a config
/// and where it is nested inside a named-field variant.
#[test]
fn test_declared_types_accept_agreeing_programs() {
    let src = r#"
        config Frame = Normal | Effect
        config Stats = { atk: Int, def: Int }
        config Card = MonsterCard { frame: Frame, stats: Stats }

        let s: Stats = Stats { atk: 1500, def: 1400 }
        let m: Card = MonsterCard { frame: Effect, stats: s }
        let n: Int = 42
    "#;
    let module = parse(src).expect("parse");
    for r in check_module(&module) {
        r.unwrap_or_else(|(n, e)| panic!("binding '{n}' should check, got {e:?}"));
    }
}

/// A named-field variant's payload is checked with the same rule as a record
/// config's — the desugaring must not create a hole.
#[test]
fn test_named_variant_field_types_are_checked() {
    let src = r#"
        config Card = MonsterCard { atk: Int }
        let bad = MonsterCard { atk: "not a number" }
    "#;
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    let (_, err) = results[0]
        .as_ref()
        .expect_err("a variant payload field must be checked");
    assert_eq!(
        *err,
        LowerError::RecordFieldTypeMismatch {
            config: "MonsterCard".to_string(),
            field: "atk".to_string(),
            declared: "Int".to_string(),
            inferred: "Str".to_string(),
        }
    );
}

/// Comparison types to `Bool`, and does so honestly: `g_cmp` asserts an
/// operation semantics the kernel does not own, so the result is capped at
/// `@Audited` exactly as arithmetic is.
#[test]
fn test_comparison_types_to_bool_at_audited() {
    let src = r#"
        config Stats = { atk: Int, def: Int }
        let a = Stats { atk: 1500, def: 1400 }
        let wins = a.atk > 1000
        let same = a.atk == a.def
    "#;
    let module = parse(src).expect("parse");
    let results = check_module(&module);

    for (i, name) in ["a", "wins", "same"].iter().enumerate() {
        let cr = results[i]
            .as_ref()
            .unwrap_or_else(|(n, e)| panic!("'{n}' should check, got {e:?}"));
        assert_eq!(&cr.name, name);
    }
    for i in [1, 2] {
        let cr = results[i].as_ref().unwrap();
        assert_eq!(
            cr.ty,
            Some(bool_ty()),
            "{} should be Bool — a two-variant sum, so a boolean match is \
             coverage-certifiable like any other sum",
            cr.name
        );
        assert_eq!(
            cr.outcome,
            Outcome::Audited,
            "{} must not claim more than g_cmp earns",
            cr.name
        );
    }
}

/// A boolean literal is discharged like the other literal introductions: it
/// establishes `HasType(true, Bool)` and asserts nothing further.
#[test]
fn test_bool_literal_earns_proven() {
    for (src, name) in [("let t = true", "t"), ("let f = false", "f")] {
        let module = parse(src).expect("parse");
        let results = check_module(&module);
        let cr = results[0]
            .as_ref()
            .unwrap_or_else(|(n, e)| panic!("{n}: {e:?}"));
        assert_eq!(cr.name, name);
        assert_eq!(cr.ty, Some(bool_ty()));
        assert_eq!(cr.outcome, Outcome::Proven);
    }
}

/// Comparison does not promote: mixing types is refused rather than silently
/// coerced, because choosing which side moves is a decision a structural split
/// may not make (ADR-0015 D-SPLIT).
#[test]
fn test_comparison_operands_must_agree() {
    let module = parse("let x = 1 > \"a\"").expect("parse");
    let results = check_module(&module);
    let (_, err) = results[0]
        .as_ref()
        .expect_err("comparing Int to Str must be refused");
    assert!(
        matches!(err, LowerError::TypeError(_)),
        "expected a type error, got {err:?}"
    );
}

/// A boolean `match` is coverage-certified like any other sum, and a missing
/// case is caught.
///
/// This is the payoff of `Bool` being a real two-variant sum rather than an
/// opaque constructor: "did you handle both cases" is the question a rules
/// engine asks constantly, and it gets a kernel certificate rather than a
/// convention.
#[test]
fn test_boolean_match_is_coverage_certified() {
    use brix_lower::CoverageOutcome;

    let src = "let a = match 1 > 2 {\n  true => 10\n  false => 20\n} proving exhaustive\n";
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    let cr = results[0].as_ref().expect("should check");
    assert_eq!(cr.coverage, Some(CoverageOutcome::Proven));

    // The typing result stays capped by `g_cmp`; coverage is a separate claim.
    assert_eq!(cr.outcome, Outcome::Audited);

    // A missing arm is a type error naming the uncovered constructor.
    let module = parse("let b = match 1 > 2 {\n  true => 10\n}\n").expect("parse");
    let results = check_module(&module);
    let (_, err) = results[0]
        .as_ref()
        .expect_err("a missing boolean case must be refused");
    assert_eq!(
        *err,
        LowerError::TypeError(TypeError::NonExhaustive(vec!["false".to_string()]))
    );
}

/// Branching on a comparison — the shape every rule in a real engine has.
#[test]
fn test_branch_on_comparison() {
    let src = r#"
        config Outcome = AttackerWins | DefenderWins

        fn resolve(atk: Int, def: Int) = match atk > def {
          true  => AttackerWins
          false => DefenderWins
        }

        let r = resolve(1800, 1500)
    "#;
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    let cr = results[0]
        .as_ref()
        .unwrap_or_else(|(n, e)| panic!("'{n}' should check, got {e:?}"));
    assert_eq!(cr.name, "r");
    assert!(
        format!("{:?}", cr.ty).contains("Outcome"),
        "expected Outcome, got {:?}",
        cr.ty
    );
}
