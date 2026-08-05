//! ADR-0012 Stage A acceptance fixtures: rule-fragment validation and plan
//! lowering (`brix_lower::l3`). No canonical encoders/identity exist yet, so
//! these tests assert on `L3PlanV1`'s structural (`PartialEq`) shape rather
//! than on a digest.

use brix_lower::{
    lower_l3_plan, L3ConfigBody, L3Limits, L3LowerError, L3PlanItem, L3PlanV1, L3TypeRef,
    L3ValueV1, L3_PROFILE_MARKER_V1,
};
use brix_syntax::parse;

fn lower(src: &str) -> Result<L3PlanV1, L3LowerError> {
    let module = parse(src).unwrap_or_else(|e| panic!("parse failed for {src:?}: {e}"));
    lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &L3Limits::generous())
}

fn lower_with_limits(src: &str, limits: &L3Limits) -> Result<L3PlanV1, L3LowerError> {
    let module = parse(src).unwrap_or_else(|e| panic!("parse failed for {src:?}: {e}"));
    lower_l3_plan(&module, L3_PROFILE_MARKER_V1, limits)
}

// ---------------------------------------------------------------------
// Whitespace/comment-insensitive source mapping.
// ---------------------------------------------------------------------

#[test]
fn whitespace_and_comments_lower_to_the_same_plan() {
    let a = r#"
        config Item = { name: Str, base: Int }
        let widget = Item { name: "widget", base: 10 }
        rule publish() = widget
    "#;
    let b = "config Item={name:Str,base:Int}\n// a widget\nlet widget=Item{base:10,name:\"widget\"}\nrule publish()=widget // publish it\n";

    let plan_a = lower(a).expect("a should lower");
    let plan_b = lower(b).expect("b should lower");
    assert_eq!(plan_a, plan_b);
}

#[test]
fn equivalent_int_and_string_spellings_normalize_to_the_same_value() {
    // "007" and "7" are the same i64; an escaped and a literal-equivalent
    // string decode to the same String.
    let a = r#"let a = 007
let b = "line\n"
"#;
    let b = "let a = 7\nlet b = \"line\n\"\n";

    let plan_a = lower(a).expect("a should lower");
    let plan_b = lower(b).expect("b should lower");
    assert_eq!(plan_a, plan_b);

    match &plan_a.items[0] {
        L3PlanItem::Let { value, .. } => assert_eq!(*value, L3ValueV1::Int(7)),
        other => panic!("expected Let, got {other:?}"),
    }
}

// ---------------------------------------------------------------------
// Reordered record-literal fields.
// ---------------------------------------------------------------------

#[test]
fn reordered_record_literal_fields_yield_declaration_ordered_value() {
    let a = r#"
        config Item = { name: Str, base: Int }
        let widget = Item { name: "widget", base: 10 }
    "#;
    let b = r#"
        config Item = { name: Str, base: Int }
        let widget = Item { base: 10, name: "widget" }
    "#;

    let plan_a = lower(a).expect("a should lower");
    let plan_b = lower(b).expect("b should lower");
    assert_eq!(plan_a, plan_b);

    match &plan_a.items[1] {
        L3PlanItem::Let { value, .. } => match value {
            L3ValueV1::Record { fields, .. } => {
                assert_eq!(fields[0].0, "name");
                assert_eq!(fields[1].0, "base");
            }
            other => panic!("expected Record, got {other:?}"),
        },
        other => panic!("expected Let, got {other:?}"),
    }
}

// ---------------------------------------------------------------------
// Config/value structure sanity (also exercises Sum/NullaryVariant).
// ---------------------------------------------------------------------

#[test]
fn sum_config_nullary_variant_lowers() {
    let src = r#"
        config Status = Open | Closed
        rule mark() = Closed
    "#;
    let plan = lower(src).expect("should lower");
    match &plan.items[1] {
        L3PlanItem::Rule {
            name,
            value,
            ordinal,
            ..
        } => {
            assert_eq!(name, "mark");
            assert_eq!(*ordinal, 0);
            assert_eq!(
                *value,
                L3ValueV1::NullaryVariant {
                    nominal_sum: "Status".to_string(),
                    variant: "Closed".to_string(),
                }
            );
        }
        other => panic!("expected Rule, got {other:?}"),
    }
}

#[test]
fn rules_canonicalize_in_module_order_not_lexical_order() {
    let src = r#"
        rule zeta() = 1
        rule alpha() = 2
    "#;
    let plan = lower(src).expect("should lower");
    let names: Vec<&str> = plan
        .items
        .iter()
        .map(|it| match it {
            L3PlanItem::Rule { name, .. } => name.as_str(),
            _ => panic!("expected Rule"),
        })
        .collect();
    assert_eq!(names, vec!["zeta", "alpha"]);
}

// ---------------------------------------------------------------------
// Duplicate rules / config members / cross-kind collisions.
// ---------------------------------------------------------------------

#[test]
fn duplicate_rule_names_reject() {
    let src = "rule f() = 1\nrule f() = 2\n";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::DuplicateItemName("f".to_string())
    );
}

#[test]
fn cross_kind_name_collision_rejects() {
    let src = "let f = 1\nrule f() = 2\n";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::DuplicateItemName("f".to_string())
    );
}

#[test]
fn duplicate_record_config_field_rejects() {
    let src = "config Item = { name: Str, name: Str }";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::DuplicateConfigMember {
            config: "Item".to_string(),
            member: "name".to_string(),
        }
    );
}

#[test]
fn duplicate_sum_config_variant_rejects() {
    let src = "config Status = Open | Open";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::DuplicateConfigMember {
            config: "Status".to_string(),
            member: "Open".to_string(),
        }
    );
}

// ---------------------------------------------------------------------
// Unclosed / forward / recursive let references.
// ---------------------------------------------------------------------

#[test]
fn unclosed_let_reference_rejects() {
    // `ghost` names nothing bindable at all: not a prior let, not a
    // constructor, not a rule.
    let src = "let a = ghost\n";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::UnclosedReference("ghost".to_string())
    );
}

#[test]
fn forward_let_reference_rejects() {
    let src = "let a = b\nlet b = 1\n";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::ForwardLetReference("b".to_string())
    );
}

#[test]
fn recursive_let_reference_rejects() {
    let src = "let a = a\n";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::RecursiveLetReference("a".to_string())
    );
}

#[test]
fn prior_closed_let_reference_is_accepted() {
    let src = "let a = 1\nlet b = a\n";
    let plan = lower(src).expect("prior let reference should be accepted");
    match &plan.items[1] {
        L3PlanItem::Let { name, value } => {
            assert_eq!(name, "b");
            assert_eq!(*value, L3ValueV1::Int(1));
        }
        other => panic!("expected Let, got {other:?}"),
    }
}

// ---------------------------------------------------------------------
// Recursive / mutually recursive configs.
// ---------------------------------------------------------------------

#[test]
fn directly_recursive_config_rejects() {
    let src = "config Nat = Zero | Succ(Nat)";
    match lower(src).unwrap_err() {
        L3LowerError::RecursiveConfig { cycle } => {
            assert!(cycle.contains(&"Nat".to_string()));
        }
        other => panic!("expected RecursiveConfig, got {other:?}"),
    }
}

#[test]
fn mutually_recursive_configs_reject() {
    let src = "config A = { x: B }\nconfig B = { y: A }\n";
    match lower(src).unwrap_err() {
        L3LowerError::RecursiveConfig { cycle } => {
            assert!(cycle.contains(&"A".to_string()));
            assert!(cycle.contains(&"B".to_string()));
        }
        other => panic!("expected RecursiveConfig, got {other:?}"),
    }
}

#[test]
fn non_recursive_nested_config_reference_is_accepted() {
    let src = r#"
        config Money = { amount: Int }
        config Item = { name: Str, price: Money }
        let widget = Item { name: "widget", price: Money { amount: 10 } }
    "#;
    let plan = lower(src).expect("non-recursive nested config should be accepted");
    match &plan.items[2] {
        L3PlanItem::Let { value, .. } => match value {
            L3ValueV1::Record {
                nominal_config,
                fields,
            } => {
                assert_eq!(nominal_config, "Item");
                assert_eq!(fields[1].0, "price");
                assert_eq!(
                    fields[1].1,
                    L3ValueV1::Record {
                        nominal_config: "Money".to_string(),
                        fields: vec![("amount".to_string(), L3ValueV1::Int(10))],
                    }
                );
            }
            other => panic!("expected Record, got {other:?}"),
        },
        other => panic!("expected Let, got {other:?}"),
    }
}

// ---------------------------------------------------------------------
// Unknown / unsupported type names.
// ---------------------------------------------------------------------

#[test]
fn unknown_type_name_rejects() {
    let src = "config Item = { name: Bogus }";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::UnknownTypeName("Bogus".to_string())
    );
}

#[test]
fn unsupported_float_field_type_rejects() {
    let src = "config Item = { weight: Float }";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::UnsupportedPrimitiveType("Float".to_string())
    );
}

// ---------------------------------------------------------------------
// Calls, arithmetic, matches, field access in a rule body.
// ---------------------------------------------------------------------

#[test]
fn call_in_rule_body_rejects() {
    let src = "rule r() = f(1)\n";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::CallNotAllowed("f".to_string())
    );
}

#[test]
fn arithmetic_in_rule_body_rejects() {
    let src = "rule r() = 1 + 2\n";
    assert_eq!(lower(src).unwrap_err(), L3LowerError::ArithmeticNotAllowed);
}

#[test]
fn match_in_rule_body_rejects() {
    let src = "config Status = Open | Closed\nrule r() = match Open { Open => 1  Closed => 2 }\n";
    assert_eq!(lower(src).unwrap_err(), L3LowerError::MatchNotAllowed);
}

#[test]
fn field_access_in_rule_body_rejects() {
    let src = r#"
        config Item = { name: Str }
        let widget = Item { name: "widget" }
        rule r() = widget.name
    "#;
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::FieldAccessNotAllowed("name".to_string())
    );
}

#[test]
fn witness_composition_in_rule_body_rejects() {
    let src = "rule r() = 1 then 2\n";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::WitnessCompositionNotAllowed
    );
}

// ---------------------------------------------------------------------
// Payload-bearing constructors.
// ---------------------------------------------------------------------

#[test]
fn payload_bearing_constructor_call_rejects() {
    let src = "config Nat = Zero | Succ(Int)\nrule r() = Succ(1)\n";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::PayloadBearingConstructor("Succ".to_string())
    );
}

#[test]
fn bare_payload_bearing_constructor_reference_rejects() {
    let src = "config Nat = Zero | Succ(Int)\nrule r() = Succ\n";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::PayloadBearingConstructor("Succ".to_string())
    );
}

#[test]
fn ambiguous_constructor_reference_rejects() {
    let src = "config A = X | Y\nconfig B = X | Z\nrule r() = X\n";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::AmbiguousConstructorReference("X".to_string())
    );
}

// ---------------------------------------------------------------------
// Float literals / integer overflow.
// ---------------------------------------------------------------------

#[test]
fn float_literal_rejects() {
    let src = "rule r() = 1.5\n";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::FloatLiteralNotAllowed("1.5".to_string())
    );
}

#[test]
fn integer_overflow_rejects() {
    // i64::MAX is 9223372036854775807; one past it must overflow.
    let src = "rule r() = 9223372036854775808\n";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::IntegerOverflow("9223372036854775808".to_string())
    );
}

// ---------------------------------------------------------------------
// Missing / extra / duplicate record fields.
// ---------------------------------------------------------------------

#[test]
fn missing_record_field_rejects() {
    let src = r#"
        config Item = { name: Str, base: Int }
        let widget = Item { name: "widget" }
    "#;
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::MissingRecordField {
            config: "Item".to_string(),
            field: "base".to_string(),
        }
    );
}

#[test]
fn extra_record_field_rejects() {
    let src = r#"
        config Item = { name: Str }
        let widget = Item { name: "widget", base: 10 }
    "#;
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::UnknownRecordField {
            config: "Item".to_string(),
            field: "base".to_string(),
        }
    );
}

#[test]
fn duplicate_record_literal_field_rejects() {
    let src = r#"
        config Item = { name: Str }
        let widget = Item { name: "a", name: "b" }
    "#;
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::DuplicateRecordLiteralField {
            config: "Item".to_string(),
            field: "name".to_string(),
        }
    );
}

// ---------------------------------------------------------------------
// Unsupported top-level items (no silent omission).
// ---------------------------------------------------------------------

#[test]
fn fn_item_rejects() {
    let src = "fn id(n) = n\n";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::FnItemNotAllowed("id".to_string())
    );
}

#[test]
fn regime_item_rejects() {
    let src = "regime r { gen f() = 1 }\n";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::RegimeItemNotAllowed("r".to_string())
    );
}

#[test]
fn show_item_rejects() {
    let src = "show 1\n";
    assert_eq!(lower(src).unwrap_err(), L3LowerError::ShowItemNotAllowed);
}

#[test]
fn witness_item_rejects() {
    let src = "witness w = 1\n";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::WitnessItemNotAllowed("w".to_string())
    );
}

#[test]
fn parameterized_rule_rejects() {
    let src = "rule r(n) = 1\n";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::ParameterizedRule("r".to_string())
    );
}

#[test]
fn prove_in_rule_body_rejects() {
    let src = "rule r() = prove 1\n";
    assert_eq!(lower(src).unwrap_err(), L3LowerError::ProveNotAllowed);
}

#[test]
fn why_in_rule_body_rejects() {
    let src = "rule r() = why(1)\n";
    assert_eq!(lower(src).unwrap_err(), L3LowerError::WhyNotAllowed);
}

#[test]
fn audit_in_rule_body_rejects() {
    let src = "rule r() = audit 1\n";
    assert_eq!(lower(src).unwrap_err(), L3LowerError::AuditNotAllowed);
}

// ---------------------------------------------------------------------
// Grade assertions (ambiguity fail-closed).
// ---------------------------------------------------------------------

#[test]
fn grade_assertion_on_let_type_rejects() {
    let src = "let a: Int @Proven = 1\n";
    match lower(src).unwrap_err() {
        L3LowerError::GradeAssertionUnsupported(_) => {}
        other => panic!("expected GradeAssertionUnsupported, got {other:?}"),
    }
}

// ---------------------------------------------------------------------
// Declared type checking.
// ---------------------------------------------------------------------

#[test]
fn declared_type_mismatch_rejects() {
    let src = "let a: Str = 1\n";
    assert_eq!(
        lower(src).unwrap_err(),
        L3LowerError::DeclaredTypeMismatch {
            name: "a".to_string(),
            expected: format!("{:?}", L3TypeRef::Str),
            found: format!("{:?}", L3TypeRef::Int),
        }
    );
}

#[test]
fn declared_type_match_is_accepted() {
    let src = "let a: Int = 1\nrule r(): Int = a\n";
    let plan = lower(src).expect("declared type should match");
    assert_eq!(plan.items.len(), 2);
}

// ---------------------------------------------------------------------
// Profile mismatch.
// ---------------------------------------------------------------------

#[test]
fn profile_mismatch_rejects_before_any_other_validation() {
    let module = parse("fn totally_bogus_and_unrelated(n) = n\n").expect("parse");
    let err = lower_l3_plan(&module, "brix.l3.other@1", &L3Limits::generous()).unwrap_err();
    assert_eq!(
        err,
        L3LowerError::ProfileMismatch {
            expected: L3_PROFILE_MARKER_V1.to_string(),
            found: "brix.l3.other@1".to_string(),
        }
    );
}

// ---------------------------------------------------------------------
// Limit failures occur before any engine/journal construction: in this
// validation-only slice, that means simply "during validation" since there
// is no engine/journal at all yet.
// ---------------------------------------------------------------------

#[test]
fn rule_count_limit_rejects() {
    let src = "rule a() = 1\nrule b() = 2\n";
    let limits = L3Limits {
        max_selected_rules: 1,
        ..L3Limits::generous()
    };
    assert_eq!(
        lower_with_limits(src, &limits).unwrap_err(),
        L3LowerError::RuleCountExceeded {
            limit: 1,
            actual: 2,
        }
    );
}

#[test]
fn config_node_limit_rejects() {
    // config Item = { a: Int, b: Int } is 1 (decl) + 2 (fields) = 3 nodes.
    let src = "config Item = { a: Int, b: Int }\n";
    let limits = L3Limits {
        max_config_nodes: 2,
        ..L3Limits::generous()
    };
    match lower_with_limits(src, &limits).unwrap_err() {
        L3LowerError::ConfigNodeLimitExceeded { limit, .. } => assert_eq!(limit, 2),
        other => panic!("expected ConfigNodeLimitExceeded, got {other:?}"),
    }
}

#[test]
fn value_node_limit_rejects() {
    let src = r#"
        config Item = { a: Int, b: Int }
        let widget = Item { a: 1, b: 2 }
    "#;
    // The record widget has 3 nodes (record + 2 ints); cap below that.
    let limits = L3Limits {
        max_total_value_nodes: 2,
        ..L3Limits::generous()
    };
    match lower_with_limits(src, &limits).unwrap_err() {
        L3LowerError::ValueNodeLimitExceeded { limit, .. } => assert_eq!(limit, 2),
        other => panic!("expected ValueNodeLimitExceeded, got {other:?}"),
    }
}

#[test]
fn value_node_limit_recounts_substituted_let_occurrences() {
    // `a` costs 1 node; each of b/c/d re-embeds it in full, so the running
    // total after a, b, c is 1 + 1 + 1 = 3, and d (the 4th occurrence)
    // pushes the total to 4.
    let src = "let a = 1\nlet b = a\nlet c = a\nlet d = a\n";
    let limits = L3Limits {
        max_total_value_nodes: 3,
        ..L3Limits::generous()
    };
    match lower_with_limits(src, &limits).unwrap_err() {
        L3LowerError::ValueNodeLimitExceeded { limit, actual } => {
            assert_eq!(limit, 3);
            assert_eq!(actual, 4);
        }
        other => panic!("expected ValueNodeLimitExceeded, got {other:?}"),
    }
}

#[test]
fn value_byte_limit_rejects() {
    let src = r#"let a = "hello world"
"#;
    let limits = L3Limits {
        max_total_value_bytes: 3,
        ..L3Limits::generous()
    };
    match lower_with_limits(src, &limits).unwrap_err() {
        L3LowerError::ValueByteLimitExceeded { limit, .. } => assert_eq!(limit, 3),
        other => panic!("expected ValueByteLimitExceeded, got {other:?}"),
    }
}

#[test]
fn value_depth_limit_rejects() {
    let src = r#"
        config Money = { amount: Int }
        config Item = { name: Str, price: Money }
        let widget = Item { name: "widget", price: Money { amount: 10 } }
    "#;
    // widget's tree: Record(Item) -> [Str, Record(Money) -> [Int]] = depth 3.
    let limits = L3Limits {
        max_value_depth: 2,
        ..L3Limits::generous()
    };
    match lower_with_limits(src, &limits).unwrap_err() {
        L3LowerError::ValueDepthExceeded { limit, actual } => {
            assert_eq!(limit, 2);
            assert_eq!(actual, 3);
        }
        other => panic!("expected ValueDepthExceeded, got {other:?}"),
    }
}

// ---------------------------------------------------------------------
// Cross-interner-order equivalent: not applicable to this slice (no
// interner exists yet), but we still assert that plan construction order
// (module scan order) does not leak into value/field ordering beyond
// declaration order, which the earlier reordering test already covers.
// ---------------------------------------------------------------------

#[test]
fn config_body_structure_matches_declaration() {
    let src = "config Item = { name: Str, base: Int }";
    let plan = lower(src).expect("should lower");
    match &plan.items[0] {
        L3PlanItem::Config(decl) => {
            assert_eq!(decl.name, "Item");
            match &decl.body {
                L3ConfigBody::Record(fields) => {
                    assert_eq!(
                        fields,
                        &vec![
                            ("name".to_string(), L3TypeRef::Str),
                            ("base".to_string(), L3TypeRef::Int),
                        ]
                    );
                }
                other => panic!("expected Record body, got {other:?}"),
            }
        }
        other => panic!("expected Config, got {other:?}"),
    }
}
