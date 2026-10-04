//! Tests for relational syntax and explicit export modifier (ADR-0046).

use brix_syntax::ast::*;
use brix_syntax::parse;

#[test]
fn test_parse_rel_input_single_key() {
    let src = r#"
rel input order: { id: Str, customer: Str, sku: Str, qty: Int, status: Str } key id
"#;
    let module = parse(src).expect("should parse rel input");
    assert_eq!(module.items.len(), 1);
    match &module.items[0] {
        Item::RelInput(decl) => {
            assert_eq!(decl.name, "order");
            assert_eq!(decl.key_fields, vec!["id"]);
            match &decl.ty {
                Ty::Record(fields) => {
                    assert_eq!(fields.len(), 5);
                    assert_eq!(fields[0].name, "id");
                    assert_eq!(fields[3].name, "qty");
                }
                other => panic!("expected record type, got {:?}", other),
            }
        }
        other => panic!("expected RelInput, got {:?}", other),
    }
}

#[test]
fn test_parse_rel_input_composite_key() {
    let src = r#"
rel input order_item: { order_id: Str, item_id: Str, qty: Int } key (order_id, item_id)
"#;
    let module = parse(src).expect("should parse composite key");
    assert_eq!(module.items.len(), 1);
    match &module.items[0] {
        Item::RelInput(decl) => {
            assert_eq!(decl.name, "order_item");
            assert_eq!(decl.key_fields, vec!["order_id", "item_id"]);
        }
        other => panic!("expected RelInput, got {:?}", other),
    }
}

#[test]
fn test_parse_rel_derived_join_and_filter() {
    let src = r#"
rel derived fulfillment =
    select { order_id: o.id, customer: o.customer, sku: o.sku, qty: o.qty }
    from o in order, b in backorder
    where o.sku == b.sku and b.available >= o.qty and o.status == "pending"
"#;
    let module = parse(src).expect("should parse rel derived");
    assert_eq!(module.items.len(), 1);
    match &module.items[0] {
        Item::RelDerived(decl) => {
            assert_eq!(decl.name, "fulfillment");
            assert_eq!(decl.query.from.len(), 2);
            assert_eq!(decl.query.from[0].var, "o");
            assert_eq!(decl.query.from[0].relation, "order");
            assert_eq!(decl.query.from[1].var, "b");
            assert_eq!(decl.query.from[1].relation, "backorder");
            assert!(decl.query.where_clause.is_some());
            assert!(decl.query.group_by.is_empty());
            match &decl.query.select {
                Expr::AnonRecord(fields) => {
                    assert_eq!(fields.len(), 4);
                    assert_eq!(fields[0].0, "order_id");
                }
                other => panic!("expected AnonRecord select, got {:?}", other),
            }
        }
        other => panic!("expected RelDerived, got {:?}", other),
    }
}

#[test]
fn test_parse_rel_derived_group_by_and_count() {
    let src = r#"
rel derived backordered_sku_count =
    select { sku: b.sku, pending_orders: count() }
    from o in order, b in backorder
    where o.sku == b.sku and b.available < o.qty
    group by b.sku
"#;
    let module = parse(src).expect("should parse rel derived with group by");
    assert_eq!(module.items.len(), 1);
    match &module.items[0] {
        Item::RelDerived(decl) => {
            assert_eq!(decl.name, "backordered_sku_count");
            assert_eq!(decl.query.group_by.len(), 1);
            match &decl.query.group_by[0] {
                Expr::Field(base, field) => {
                    assert_eq!(*base, Box::new(Expr::Var("b".to_string())));
                    assert_eq!(field, "sku");
                }
                other => panic!("expected field access in group by, got {:?}", other),
            }
            match &decl.query.select {
                Expr::AnonRecord(fields) => {
                    assert_eq!(fields.len(), 2);
                    assert_eq!(fields[0].0, "sku");
                    assert_eq!(fields[1].0, "pending_orders");
                    match &fields[1].1 {
                        Expr::Call { func, args } => {
                            assert_eq!(func, "count");
                            assert!(args.is_empty());
                        }
                        other => panic!("expected count() call, got {:?}", other),
                    }
                }
                other => panic!("expected AnonRecord select, got {:?}", other),
            }
        }
        other => panic!("expected RelDerived, got {:?}", other),
    }
}

#[test]
fn test_export_modifier() {
    let src = r#"
export fn helper(x: Int): Int = x + 1
export config Status = Active | Inactive
export rel input order: { id: Str } key id
export rel derived pending = select { id: o.id } from o in order
"#;
    let module = parse(src).expect("should parse export declarations");
    assert_eq!(module.items.len(), 4);
    for item in &module.items {
        match item {
            Item::Export(inner) => match inner.as_ref() {
                Item::Fn(f) => assert_eq!(f.name, "helper"),
                Item::Config(c) => assert_eq!(c.name, "Status"),
                Item::RelInput(r) => assert_eq!(r.name, "order"),
                Item::RelDerived(r) => assert_eq!(r.name, "pending"),
                other => panic!("unexpected exported item {:?}", other),
            },
            other => panic!("expected Export, got {:?}", other),
        }
    }
}

#[test]
fn test_contextual_keywords_as_fields_and_identifiers() {
    let src = r#"
config KeyMeta = { key: Str, group: Str, by: Str }
fn get_key(m: KeyMeta): Str = m.key
fn get_group(m: KeyMeta): Str = m.group
fn get_by(m: KeyMeta): Str = m.by
let key = "primary"
let group = "admin"
let by = "user"
"#;
    let module = parse(src).expect("should parse contextual keywords key, group, by");
    assert_eq!(module.items.len(), 7);
}

#[test]
fn test_qualified_names() {
    let src = r#"
use store::inventory
fn check(x: store::Order): Bool = store::is_valid(x)
let order = store::Order { id: "1" }
"#;
    let module = parse(src).expect("should parse qualified names");
    assert_eq!(module.items.len(), 3);
}

#[test]
fn test_semicolon_bearing_rel_fails() {
    let src = "rel input order: { id: Str } key id;";
    let err = parse(src).expect_err("trailing semicolon must fail in Brix");
    assert!(err.message.contains("Unexpected character ';'"));

    let src2 = "rel derived d = select { id: o.id } from o in order;";
    let err2 = parse(src2).expect_err("trailing semicolon must fail in Brix");
    assert!(err2.message.contains("Unexpected character ';'"));
}

#[test]
fn test_legacy_identifiers_and_contextual_keywords() {
    let src = r#"
let select = 1
let export = 2
let rel = 3
let key = 4
let group = 5
let by = 6

fn f(select: Int): Int = select
fn g(export: Int): Int = export
fn h(rel: Int): Int = rel

config T = { rel: Int, select: Str, export: Bool }
fn make_t(): T = { rel: 10, select: "text", export: true }
"#;
    let module = parse(src).expect("legacy identifiers and contextual keywords must parse cleanly");
    assert_eq!(module.items.len(), 11);
}

#[test]
fn test_export_chain_bounded_and_no_stack_overflow() {
    use brix_syntax::{parse_bounded, ParseLimits};

    // 20,000 chained exports under strict limits must immediately fail with ParseError without stack overflow
    let src = format!("{}config X = A", "export ".repeat(20_000));
    let err = parse_bounded(&src, ParseLimits::strict())
        .expect_err("chained exports must be rejected safely");
    assert!(
        err.message.contains("redundant 'export' modifier")
            || err.message.contains("limit exceeded"),
        "error message should indicate redundant modifier or limit exceeded, got: {:?}",
        err.message
    );
}
