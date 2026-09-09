//! Tests for surface syntax of `input` declarations (ADR-0031).

use brix_syntax::ast::*;
use brix_syntax::parse;

#[test]
fn test_input_scalar_types() {
    let source = r#"
input threshold: Int
input enabled: Bool
input label: Str
"#;
    let module = parse(source).expect("valid input declarations should parse");
    assert_eq!(module.items.len(), 3);

    match &module.items[0] {
        Item::Input(InputDecl { name, ty }) => {
            assert_eq!(name, "threshold");
            assert_eq!(*ty, Ty::Named("Int".into()));
        }
        other => panic!("Expected Item::Input, got {:?}", other),
    }

    match &module.items[1] {
        Item::Input(InputDecl { name, ty }) => {
            assert_eq!(name, "enabled");
            assert_eq!(*ty, Ty::Named("Bool".into()));
        }
        other => panic!("Expected Item::Input, got {:?}", other),
    }

    match &module.items[2] {
        Item::Input(InputDecl { name, ty }) => {
            assert_eq!(name, "label");
            assert_eq!(*ty, Ty::Named("Str".into()));
        }
        other => panic!("Expected Item::Input, got {:?}", other),
    }
}

#[test]
fn test_input_no_semicolon_enforcement() {
    let source = "input threshold: Int;";
    let err = parse(source);
    assert!(err.is_err(), "trailing semicolon must be rejected");
}

#[test]
fn test_input_missing_type_fails() {
    let source = "input threshold:";
    let err = parse(source);
    assert!(err.is_err(), "missing type must be rejected");
}

#[test]
fn test_input_missing_name_fails() {
    let source = "input : Int";
    let err = parse(source);
    assert!(err.is_err(), "missing name must be rejected");
}

#[test]
fn test_input_coexists_with_program_items() {
    let source = r#"
config Decision = Expedite | Ship | Hold

input stock: Int
input threshold: Int

rule base_stock() = stock

propose expedite(stock) priority 5 when stock >= 50 = Expedite
propose ship(stock, threshold) priority 10 when stock >= threshold = Ship
propose hold() priority 100 when true = Hold

commit shipping_decision from (expedite, ship, hold)
"#;
    let module = parse(source).expect("program with input declarations should parse");
    assert_eq!(module.items.len(), 8);

    let mut input_names = Vec::new();
    for item in &module.items {
        if let Item::Input(decl) = item {
            input_names.push(decl.name.as_str());
        }
    }
    assert_eq!(input_names, vec!["stock", "threshold"]);
}
