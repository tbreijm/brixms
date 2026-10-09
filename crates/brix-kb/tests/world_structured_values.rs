//! Tests for R1: Structured world values end-to-end (nominal records and sum variants).
use std::collections::BTreeMap;

use brix_kb::world::{
    decision_codec::{decision_tuple, decode_settled_decision, Value},
    reference, TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldNetwork,
};
use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};

fn link_world(sources: &[(&str, &str)]) -> (brix_lower::module_graph::LinkedProgram, WorldNetwork) {
    let mut map = BTreeMap::new();
    for (name, src) in sources {
        map.insert(name.to_string(), src.to_string());
    }
    let loader = |name: &str| map.get(name).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .expect("module graph load failed");
    let linked = graph.link().expect("module graph link failed");
    let net = WorldNetwork::from_program(&linked).expect("WorldNetwork::from_program failed");
    (linked, net)
}

#[test]
fn test_r1_structured_decision_values_and_input_records() {
    let schema_pkg = r#"
export config Status = Pending | Approved(Int) | Rejected
export config Event = { tag: Str, code: Int }
"#;

    let root_pkg = r#"
use schema
rel input orders: { id: Str, event: schema::Event, status: schema::Status } key id

decide order_decision for o in orders per id {
    propose approved priority 10 when (match o.status {
        schema::Status::Approved(prio) => prio > 0
        schema::Status::Pending => false
        schema::Status::Rejected => false
    }) = schema::Status::Approved(100)

    propose rejected priority 50 when true = schema::Status::Rejected
}
"#;

    let (linked, mut net) = link_world(&[("schema", schema_pkg), ("root", root_pkg)]);

    // Construct a batch inserting an order record with structured fields
    let mut rec = TupleRecord::new();
    rec.set_str("id", "ORD-1");
    rec.set_str("event", r#"{"type":"record","nominal":"schema::Event","fields":[{"name":"tag","value":{"type":"string","value":"order_created"}},{"name":"code","value":{"type":"int","value":"1"}}]}"#);
    rec.set_str("status", r#"{"type": "sum", "nominal": "schema::Status", "variant": "Approved", "args": [{"type": "int", "value": "5"}]}"#);

    let batch = WorldBatch::new(
        0,
        "batch-1",
        vec![WorldBatchOp::Upsert {
            relation: "root::orders".to_string(),
            key: WorldKey::from_u64(1),
            tuple: rec.to_tuple(),
        }],
    );

    let report = net.apply_batch(&batch).expect("apply_batch failed");

    // Settle entity ORD-1 in root::order_decision
    let settlement = report
        .settlements
        .get("root::order_decision")
        .and_then(|m| m.get("ORD-1"))
        .expect("ORD-1 should have a settled decision");

    assert_eq!(settlement.candidate_name, "approved");
    assert_eq!(settlement.priority, 10);

    // Assert structured value on settlement
    let expected_val = Value::Variant {
        nominal: "schema::Status".to_string(),
        variant: "Approved".to_string(),
        args: vec![Value::Int(100)],
    };
    assert_eq!(settlement.value, expected_val);

    // Verify to_json outputs structured JSON, not stringified string
    let json_val = settlement.to_json();
    assert_eq!(
        json_val["value"],
        serde_json::json!({
            "type": "sum",
            "nominal": "schema::Status",
            "variant": "Approved",
            "args": [{"type":"int","value":"100"}],
        })
    );

    // Test explain_decision_for shows both eligible losers and guard-false candidates
    let explanation = net
        .explain_decision_for("root::order_decision", "ORD-1")
        .expect("explanation found");
    assert_eq!(explanation.winning_candidate.as_deref(), Some("approved"));
    assert_eq!(explanation.candidates.len(), 2);
    let approved_cand = explanation
        .candidates
        .iter()
        .find(|c| c.name == "approved")
        .unwrap();
    assert!(approved_cand.winning);
    assert_eq!(approved_cand.supports_count, 1);
    let rejected_cand = explanation
        .candidates
        .iter()
        .find(|c| c.name == "rejected")
        .unwrap();
    assert!(!rejected_cand.winning);
    assert_eq!(rejected_cand.supports_count, 1); // eligible loser

    // Test canonical encoding and roundtrip of settlement tuple
    let tuple = decision_tuple(settlement);
    let decoded = decode_settled_decision("ORD-1", tuple.as_bytes()).expect("decode settlement");
    assert_eq!(decoded.value, expected_val);
    assert_eq!(decoded.candidate_name, "approved");

    // Independent reference evaluator agreement
    let ref_prog = reference::from_program(&linked).expect("ref program");
    let base_relations = net.current_state().base_relations;
    let ref_state = reference::evaluate(&ref_prog, &base_relations).expect("ref evaluate");
    let ref_settlement = ref_state
        .settlements
        .get("root::order_decision")
        .and_then(|m| m.get("ORD-1"))
        .expect("ref settlement for ORD-1");
    assert_eq!(ref_settlement.candidate_name, "approved");
    assert_eq!(ref_settlement.priority, 10);
    assert_eq!(
        ref_settlement.value,
        reference::Value::Variant {
            nominal: "schema::Status".to_string(),
            variant: "Approved".to_string(),
            args: vec![reference::Value::Int(100)],
        }
    );
}

#[test]
fn test_r1_bare_and_qualified_constructors_across_packages() {
    let p1 = r#"
export config Mode = Fast | Slow
"#;

    let p2 = r#"
export config Mode = Fast | Slow
"#;

    let root_ambiguous = r#"
use p1
use p2

rel input dummy: { id: Str } key id

decide d for x in dummy per id {
    propose p priority 1 when true = Fast
}
"#;

    // Linking ambiguous bare constructor 'Fast' must be rejected at use-site
    let mut map = BTreeMap::new();
    map.insert("p1".to_string(), p1.to_string());
    map.insert("p2".to_string(), p2.to_string());
    map.insert("root".to_string(), root_ambiguous.to_string());
    let loader = |name: &str| map.get(name).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).unwrap();
    let msg = graph.link().unwrap_err().to_string();
    assert!(
        msg.contains("ambiguous symbol 'Fast'")
            && msg.contains("p1::Mode::Fast")
            && msg.contains("p2::Mode::Fast"),
        "expected ambiguous constructor diagnostic with alternatives, got: {msg}"
    );

    // Qualification with p1::Fast or p1::Mode::Fast must succeed
    let root_qualified = r#"
use p1
use p2

rel input dummy: { id: Str } key id

decide d for x in dummy per id {
    propose p priority 1 when true = p1::Mode::Fast
}
"#;
    let (_, _net) = link_world(&[("p1", p1), ("p2", p2), ("root", root_qualified)]);
}

#[test]
fn test_r1_input_type_validation_rejects_malformed_values() {
    let schema_pkg = r#"
export config Decision = Approved(Int) | Denied
"#;

    let root_pkg = r#"
use schema

rel input items: {
    id: Str,
    decision: schema::Decision,
} key id

decide test_decide for i in items per id {
    propose p priority 1 when true = 1
}
"#;

    let (_, mut net) = link_world(&[("schema", schema_pkg), ("root", root_pkg)]);

    // 1. Wrong payload type (string instead of int)
    let mut rec1 = TupleRecord::new();
    rec1.set_str("id", "I-1");
    rec1.set_str("decision", r#"{"type": "sum", "nominal": "schema::Decision", "variant": "Approved", "args": [{"type": "string", "value": "not_an_int"}]}"#);
    let batch1 = WorldBatch::new(
        1,
        "b1",
        vec![WorldBatchOp::Upsert {
            relation: "root::items".to_string(),
            key: WorldKey::from_u64(1),
            tuple: rec1.to_tuple(),
        }],
    );
    assert!(
        net.apply_batch(&batch1).is_err(),
        "should reject string payload when Int is expected"
    );

    // 2. Wrong arity (2 args instead of 1)
    let mut rec2 = TupleRecord::new();
    rec2.set_str("id", "I-2");
    rec2.set_str("decision", r#"{"type": "sum", "nominal": "schema::Decision", "variant": "Approved", "args": [{"type": "int", "value": "1"}, {"type": "int", "value": "2"}]}"#);
    let batch2 = WorldBatch::new(
        2,
        "b2",
        vec![WorldBatchOp::Upsert {
            relation: "root::items".to_string(),
            key: WorldKey::from_u64(2),
            tuple: rec2.to_tuple(),
        }],
    );
    assert!(
        net.apply_batch(&batch2).is_err(),
        "should reject wrong arity"
    );

    // 3. Unknown tag / variant
    let mut rec3 = TupleRecord::new();
    rec3.set_str("id", "I-3");
    rec3.set_str("decision", r#"{"type": "sum", "nominal": "schema::Decision", "variant": "UnknownVariant", "args": []}"#);
    let batch3 = WorldBatch::new(
        3,
        "b3",
        vec![WorldBatchOp::Upsert {
            relation: "root::items".to_string(),
            key: WorldKey::from_u64(3),
            tuple: rec3.to_tuple(),
        }],
    );
    assert!(
        net.apply_batch(&batch3).is_err(),
        "should reject unknown variant"
    );

    // 4. Incorrect nominal type
    let mut rec4 = TupleRecord::new();
    rec4.set_str("id", "I-4");
    rec4.set_str("decision", r#"{"type": "sum", "nominal": "other::Decision", "variant": "Approved", "args": [{"type": "int", "value": "1"}]}"#);
    let batch4 = WorldBatch::new(
        4,
        "b4",
        vec![WorldBatchOp::Upsert {
            relation: "root::items".to_string(),
            key: WorldKey::from_u64(4),
            tuple: rec4.to_tuple(),
        }],
    );
    assert!(
        net.apply_batch(&batch4).is_err(),
        "should reject mismatched nominal type"
    );
}

#[test]
fn structured_transport_rejects_wrong_tags_duplicates_and_oversized_frames() {
    use brix_canon::CanonWriter;
    use brix_lower::l3_v2::{L3Schema, L3SchemaBody, L3SchemaType};
    use brix_syntax::ast::Ty;
    let schemas = BTreeMap::from([
        (
            "root::R".into(),
            L3Schema {
                name: "root::R".into(),
                body: L3SchemaBody::Record(BTreeMap::from([
                    ("a".into(), L3SchemaType::Int),
                    ("b".into(), L3SchemaType::Int),
                ])),
            },
        ),
        (
            "root::S".into(),
            L3Schema {
                name: "root::S".into(),
                body: L3SchemaBody::Sum(vec![
                    ("Some".into(), vec![L3SchemaType::Int]),
                    ("None".into(), vec![]),
                ]),
            },
        ),
    ]);
    for input in [
        r#"{"type":"sum","nominal":"root::S","variant":"Some","args":[{"type":"string","value":"1"}]}"#,
        r#"{"type":"sum","nominal":"root::S","variant":"None","args":[],"extra":0}"#,
        r#"{"type":"sum","nominal":"root::S","variant":"None","variant":"Some","args":[]}"#,
        r#"{"type":"sum","nominal":"root::S","variant":"None","args":{}}"#,
    ] {
        assert!(
            Value::from_typed_bytes_with_schemas(
                input.as_bytes(),
                &Ty::Named("root::S".into()),
                &schemas
            )
            .is_err(),
            "accepted {input}"
        );
    }
    // A string that looks like a tagged integer stays exactly that string.
    let text = r#"{"type":"int","value":"12"}"#;
    assert_eq!(
        Value::from_typed_bytes(text.as_bytes(), &Ty::Named("Str".into())).unwrap(),
        Value::Str(text.into())
    );
    for names in [["a", "a"], ["b", "a"]] {
        let mut w = CanonWriter::new();
        w.write_uint(6);
        w.write_ident("root::R");
        w.write_uint(2);
        for name in names {
            w.write_ident(name);
            w.write_uint(1);
            w.write_int(1);
        }
        let mut bytes = b"\0brix.world.value@1\0".to_vec();
        bytes.extend(w.finish());
        assert!(Value::from_typed_bytes_with_schemas(
            &bytes,
            &Ty::Named("root::R".into()),
            &schemas
        )
        .is_err());
    }
    let mut w = CanonWriter::new();
    w.write_uint(7);
    w.write_ident("root::S");
    w.write_ident("Some");
    w.write_uint(u64::MAX);
    let mut bytes = b"\0brix.world.value@1\0".to_vec();
    bytes.extend(w.finish());
    assert!(
        Value::from_typed_bytes_with_schemas(&bytes, &Ty::Named("root::S".into()), &schemas)
            .is_err()
    );
}

#[test]
fn variant_decisions_and_derived_fields_agree_with_reference_under_mutation() {
    let source = r#"
config Rec = { id: Str, amt: Int }
config Decision = Approved(Rec) | Rejected
fn accepted(d: Decision): Bool = match d { Approved(_) => true Rejected => false }
rel input orders: { id: Str, status: Decision } key id
rel derived eligible = select { id:o.id, status:o.status } from o in orders where accepted(o.status)
decide decision for o in orders per id { propose current priority 1 when true = o.status }
decide approved for o in eligible per id { propose current priority 1 when true = o.status }
"#;
    for seed in [1u64, 73, 4099] {
        let (linked, mut net) = link_world(&[("root", source)]);
        let oracle = reference::from_program(&linked).unwrap();
        let mut rng = seed;
        for step in 0..100 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let id = format!("order-{}", rng % 9);
            let key = WorldKey::from_str(&id);
            let op = if rng % 5 == 0 {
                WorldBatchOp::Remove {
                    relation: "root::orders".into(),
                    key,
                }
            } else {
                let status = if rng % 2 == 0 {
                    Value::Variant {
                        nominal: "root::Decision".into(),
                        variant: "Approved".into(),
                        args: vec![Value::Record {
                            nominal: "root::Rec".into(),
                            fields: BTreeMap::from([
                                ("id".into(), Value::Str(id.clone())),
                                ("amt".into(), Value::Int((rng % 100) as i64)),
                            ]),
                        }],
                    }
                } else {
                    Value::Variant {
                        nominal: "root::Decision".into(),
                        variant: "Rejected".into(),
                        args: vec![],
                    }
                };
                let mut tuple = TupleRecord::new();
                tuple.set_str("id", &id);
                tuple.set_str("status", &status.to_json().to_string());
                WorldBatchOp::Upsert {
                    relation: "root::orders".into(),
                    key,
                    tuple: tuple.to_tuple(),
                }
            };
            net.apply_batch(&WorldBatch::new(
                step,
                format!("seed-{seed}-step-{step}"),
                vec![op],
            ))
            .unwrap();
            let state = net.current_state();
            let expected = reference::evaluate(&oracle, &state.base_relations).unwrap();
            assert_eq!(
                state.derived_relations, expected.derived_relations,
                "seed={seed} step={step}"
            );
            assert_eq!(
                state.settlements.keys().collect::<Vec<_>>(),
                expected.settlements.keys().collect::<Vec<_>>()
            );
            for (decide, rows) in &state.settlements {
                assert_eq!(rows.len(), expected.settlements[decide].len());
                for (entity, actual) in rows {
                    let want = &expected.settlements[decide][entity];
                    assert_eq!(
                        actual.calendar_key, want.calendar_key,
                        "seed={seed} step={step}"
                    );
                    assert_eq!(format!("{:?}", actual.value), format!("{:?}", want.value));
                    let bytes = decision_tuple(actual);
                    assert_eq!(
                        decode_settled_decision(entity, bytes.as_bytes()).unwrap(),
                        *actual
                    );
                }
            }
        }
    }
}
