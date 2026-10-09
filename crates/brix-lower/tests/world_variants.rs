//! Nominal identity and lexical constructor scope in the persistent world profile.
use brix_lower::l3_v2::L3ValueV2;
use brix_lower::module_graph::{LinkedProgram, ModuleGraph, ModuleLoaderLimits, QualifiedName};
use brix_lower::world_expr::CompiledProgramEnv;
use std::collections::{BTreeMap, BTreeSet};

fn link(sources: &[(&str, &str)]) -> Result<LinkedProgram, String> {
    let sources: BTreeMap<_, _> = sources
        .iter()
        .map(|(name, source)| (*name, source.to_string()))
        .collect();
    ModuleGraph::load(
        "root",
        &|name: &str| sources.get(name).cloned(),
        ModuleLoaderLimits::default(),
    )
    .map_err(|e| e.to_string())?
    .link()
    .map_err(|e| e.to_string())
}

fn evaluate(program: &LinkedProgram, helper: &str) -> Result<L3ValueV2, String> {
    let env = CompiledProgramEnv::from_linked_program(program)?;
    let expression = brix_syntax::ast::Expr::Call {
        func: format!("root::{helper}"),
        args: vec![],
    };
    env.compile_expr(&expression, &BTreeSet::new())?
        .eval(&BTreeMap::new())
}

#[test]
fn original_record_variant_program_and_payload_match_guard() {
    let program = link(&[("root", r#"
config Rec = { id: Str, amt: Int }
config Decision = Approved(Rec) | Rejected
fn positive(d: Decision): Bool = match d { Approved(r) => r.amt > 0 Rejected => false }
fn wildcard(d: Decision): Bool = match d { Approved(_) => true Rejected => false }
fn check(): Bool = positive(Approved(Rec { id: "x", amt: 5 })) && wildcard(Rejected) == false
rel input order: { id: Str, amt: Int } key id
decide approve for o in order per id {
 propose yes priority 10 when positive(Approved(Rec { id: o.id, amt: o.amt })) = Approved(Rec { id: o.id, amt: o.amt })
 propose no priority 100 when true = Rejected
}
"#)]).unwrap();
    assert_eq!(evaluate(&program, "check").unwrap(), L3ValueV2::Bool(true));
    let env = CompiledProgramEnv::from_linked_program(&program).unwrap();
    let decide = &program.decides[&QualifiedName::new("root", "approve")];
    let row = L3ValueV2::Record {
        nominal_config: String::new(),
        fields: vec![
            ("id".into(), L3ValueV2::Str("x".into())),
            ("amt".into(), L3ValueV2::Int(5)),
        ],
    };
    let value = env
        .compile_expr(&decide.proposals[0].value, &BTreeSet::from(["o".into()]))
        .unwrap()
        .eval(&BTreeMap::from([("o".into(), row)]))
        .unwrap();
    assert!(
        matches!(value, L3ValueV2::Ctor { nominal_sum, variant, args }
        if nominal_sum == "root::Decision" && variant == "Approved" && matches!(&args[0], L3ValueV2::Record { nominal_config, .. } if nominal_config == "root::Rec"))
    );
}

#[test]
fn packages_keep_their_local_types_and_helpers_with_identical_names() {
    let program = link(&[
        (
            "root",
            "use a\nuse b\nfn check(): Int = a::amount(a::make()) + b::amount(b::make())",
        ),
        (
            "a",
            r#"config Rec = { amt: Int }
export config Decision = Approved(Rec) | Rejected
export fn make(): Decision = Approved(Rec { amt: 5 })
export fn amount(d: Decision): Int = match d { Approved(r) => r.amt Rejected => 0 }"#,
        ),
        (
            "b",
            r#"config Rec = { amt: Int }
export config Decision = Approved(Rec) | Rejected
export fn make(): Decision = Approved(Rec { amt: 7 })
export fn amount(d: Decision): Int = match d { Approved(r) => r.amt Rejected => 0 }"#,
        ),
    ])
    .unwrap();
    assert_eq!(evaluate(&program, "check").unwrap(), L3ValueV2::Int(12));
}

#[test]
fn ambiguous_bare_constructor_fails_at_use_with_sorted_alternatives() {
    let error = link(&[
        (
            "root",
            "use b\nuse a\nfn bad(): Int = match Approved(1) { Approved(x) => x }",
        ),
        ("a", "export config Decision = Approved(Int)"),
        ("b", "export config Decision = Approved(Int)"),
    ])
    .unwrap_err();
    assert!(error.contains("ambiguous symbol 'Approved'"), "{error}");
    assert!(
        error.contains("a::Decision::Approved, b::Decision::Approved"),
        "{error}"
    );
}

#[test]
fn all_qualified_constructor_spellings_resolve_and_match_nominal_owner() {
    let program = link(&[
        ("root", "use a\nfn check(): Int = match a::Decision::Approved(3) { Decision::Approved(x) => x }"),
        ("a", "export config Decision = Approved(Int)"),
    ]).unwrap();
    assert_eq!(evaluate(&program, "check").unwrap(), L3ValueV2::Int(3));
    let program = link(&[
        (
            "root",
            "use a\nuse b\nfn check(): Int = match a::Approved(3) { b::Approved(x) => x }",
        ),
        ("a", "export config Decision = Approved(Int)"),
        ("b", "export config Decision = Approved(Int)"),
    ])
    .unwrap();
    assert!(
        evaluate(&program, "check").is_err(),
        "a pattern from b cannot match a's nominal sum"
    );
}

#[test]
fn private_and_transitively_imported_constructors_are_inaccessible() {
    for spelling in ["Secret", "a::Secret", "a::Decision::Secret"] {
        let root = format!("use a\nfn bad() = {spelling}");
        let error = link(&[("root", &root), ("a", "config Decision = Secret")]).unwrap_err();
        assert!(error.contains("not exported"), "{error}");
    }
    let error = link(&[
        ("root", "use a\nfn bad() = Secret"),
        ("a", "use b\nexport fn harmless(): Int = 1"),
        ("b", "export config Decision = Secret"),
    ])
    .unwrap_err();
    assert!(error.contains("unresolved symbol"), "{error}");
}

#[test]
fn malformed_structured_results_are_rejected_before_settlement() {
    for expression in [
        "Approved(1)",
        "Approved(Rec { amt: true })",
        "Approved(Rec { wrong: 1 })",
        "Approved(Rec { amt: 1 }, Rec { amt: 2 })",
        "match Approved(true) { Approved(_) => true Rejected => false }",
        "Rec { amt: true }.amt == true",
    ] {
        let source = format!("config Rec = {{ amt: Int }}\nconfig Decision = Approved(Rec) | Rejected\nfn check() = {expression}");
        let program = link(&[("root", &source)]).unwrap();
        assert!(
            evaluate(&program, "check").is_err(),
            "accepted {expression}"
        );
    }
}
