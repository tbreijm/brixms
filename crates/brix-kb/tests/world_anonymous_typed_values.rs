//! Structural typed decoding for anonymous world query records.

use brix_kb::world::decision_codec::Value;
use brix_syntax::ast::{FieldDecl, Ty};
use std::collections::BTreeMap;

fn page_type() -> Ty {
    Ty::Record(vec![
        FieldDecl {
            name: "items".into(),
            ty: Ty::App("List".into(), vec![Ty::Named("Str".into())]),
        },
        FieldDecl {
            name: "truncated".into(),
            ty: Ty::Named("Bool".into()),
        },
    ])
}

fn page_value(items: Value, truncated: Value) -> Value {
    Value::Record {
        nominal: String::new(),
        fields: BTreeMap::from([("items".into(), items), ("truncated".into(), truncated)]),
    }
}

#[test]
fn typed_decode_accepts_anonymous_collect_take_record() {
    let value = page_value(
        Value::List(vec![Value::Str("a".into()), Value::Str("b".into())]),
        Value::Bool(true),
    );
    let bytes = value.to_bytes();
    assert_eq!(
        Value::from_typed_bytes_with_schemas(&bytes, &page_type(), &BTreeMap::new()).unwrap(),
        value
    );
}

#[test]
fn typed_decode_rejects_wrong_anonymous_record_shape_or_field_types() {
    let wrong_nominal = Value::Record {
        nominal: "SomeNominal".into(),
        fields: match page_value(Value::List(vec![]), Value::Bool(false)) {
            Value::Record { fields, .. } => fields,
            _ => unreachable!(),
        },
    };
    let extra_field = Value::Record {
        nominal: String::new(),
        fields: BTreeMap::from([
            ("items".into(), Value::List(vec![])),
            ("truncated".into(), Value::Bool(false)),
            ("extra".into(), Value::Bool(true)),
        ]),
    };
    let wrong_item = page_value(Value::Str("not a list".into()), Value::Bool(false));
    for value in [wrong_nominal, extra_field, wrong_item] {
        assert!(
            Value::from_typed_bytes_with_schemas(&value.to_bytes(), &page_type(), &BTreeMap::new())
                .is_err(),
            "accepted {value:?}"
        );
    }
}

#[test]
fn typed_decode_validates_nested_anonymous_records_inside_lists() {
    let ty = Ty::Record(vec![FieldDecl {
        name: "pages".into(),
        ty: Ty::App(
            "List".into(),
            vec![Ty::Record(vec![FieldDecl {
                name: "truncated".into(),
                ty: Ty::Named("Bool".into()),
            }])],
        ),
    }]);
    let value = Value::Record {
        nominal: String::new(),
        fields: BTreeMap::from([(
            "pages".into(),
            Value::List(vec![Value::Record {
                nominal: String::new(),
                fields: BTreeMap::from([("truncated".into(), Value::Bool(false))]),
            }]),
        )]),
    };
    assert_eq!(
        Value::from_typed_bytes_with_schemas(&value.to_bytes(), &ty, &BTreeMap::new()).unwrap(),
        value
    );
}
