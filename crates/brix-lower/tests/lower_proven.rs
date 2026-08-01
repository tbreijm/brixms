use brix_lower::{check_module, LowerError};
use brix_semantic::Outcome;
use brix_syntax::parse;
use soc_regimes::type_realization::{Ty as TrTy, TypeError};

#[test]
fn test_id_fixture_audited() {
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
    assert_eq!(check_res.outcome, Outcome::Audited);
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
fn test_record_and_field_audited() {
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
    assert_eq!(res_p.outcome, Outcome::Audited);
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
    assert_eq!(res_a.outcome, Outcome::Audited);
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
fn str_literal_earns_proven_record_and_field_stay_audited() {
    // The Str literal rests only on the discharged `g_str_lit` → Proven; the
    // record and field access use undischarged `g_record*`/`g_field` → Audited.
    let src = "let s = \"hi\"\nlet w = Item { name: \"widget\", base: 10 }\nlet n = w.name\n";
    let module = brix_syntax::parse(src).expect("parse");
    let results = brix_lower::check_module(&module);
    assert_eq!(results.len(), 3);

    let expected = [
        ("s", Outcome::Proven),
        ("w", Outcome::Audited),
        ("n", Outcome::Audited),
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
    assert_eq!(check_res.outcome, Outcome::Audited);
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
fn test_sum_match_audited() {
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
    assert_eq!(res_a.outcome, Outcome::Audited);
    assert_eq!(res_a.ty, Some(TrTy::Con("Int")));

    let res_b = results[1]
        .as_ref()
        .expect("let b should lower & type-check");
    assert_eq!(res_b.name, "b");
    assert_eq!(res_b.outcome, Outcome::Audited);
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

#[test]
fn test_recursive_sum_unregistered() {
    let src = r#"
        config Nat = Zero | Succ(Nat)
        let x = Succ(1)
    "#;
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    assert_eq!(results.len(), 1);

    let (name, err) = results[0]
        .as_ref()
        .expect_err("recursive sum should leave constructors unregistered");
    assert_eq!(name, "x");
    assert_eq!(*err, LowerError::Unresolved("Succ".to_string()));
}
