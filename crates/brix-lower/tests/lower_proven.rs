use brix_lower::{check_module, LowerError};
use brix_semantic::Outcome;
use brix_syntax::parse;
use soc_regimes::type_realization::Ty as TrTy;

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
        .expect("let r binding should lower & prove successfully");

    assert_eq!(check_res.name, "r");
    assert_eq!(check_res.outcome, Outcome::Proven);
    assert_eq!(check_res.ty, Some(TrTy::Con("Int")));
}

#[test]
fn test_let_lit_proven() {
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
        .expect("let p should lower & prove successfully");
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
        .expect("let a should lower & prove successfully");
    assert_eq!(res_a.name, "a");
    assert_eq!(res_a.outcome, Outcome::Proven);
    assert_eq!(res_a.ty, Some(TrTy::Con("Int")));
}

#[test]
fn arithmetic_int_float_and_mixed_promotion_reach_proven() {
    // Int+Int→Int, Float+Float→Float, and mixed Int+Float→Float (via the
    // Int↪Float promotion witness) all reach Proven with the expected type.
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
        assert_eq!(cr.outcome, Outcome::Proven, "{src} not Proven");
        assert_eq!(cr.ty, Some(want), "{src} wrong type");
    }
}

#[test]
fn function_using_arithmetic_reaches_proven() {
    // `n + n` defaults the parameter to Int, so `double : Int → Int` and the
    // applied result proves as Int.
    let src = "fn double(n) = n + n\nlet r = double(21)";
    let module = parse(src).expect("parse");
    let results = check_module(&module);
    assert_eq!(results.len(), 1);
    let cr = results[0].as_ref().expect("double(21) should prove");
    assert_eq!(cr.name, "r");
    assert_eq!(cr.outcome, Outcome::Proven);
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
fn str_literal_and_str_record_field_reach_proven() {
    // Str literal, a record with a Str field, and field access on it all prove.
    let src = "let s = \"hi\"\nlet w = Item { name: \"widget\", base: 10 }\nlet n = w.name\n";
    let module = brix_syntax::parse(src).expect("parse");
    let results = brix_lower::check_module(&module);
    assert_eq!(results.len(), 3);
    for r in &results {
        let cr = r
            .as_ref()
            .unwrap_or_else(|(name, e)| panic!("{name}: {e:?}"));
        assert_eq!(
            cr.outcome,
            brix_semantic::Outcome::Proven,
            "{} not Proven",
            cr.name
        );
    }
}
