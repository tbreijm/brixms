//! Qualification and regression tests for Stage P4 (Maintained operator network, TMS, and deliberation).

use std::collections::BTreeMap;

use brix_kb::world::{
    TupleRecord, Value, WorldBatch, WorldBatchOp, WorldKey, WorldNetwork, WorldTuple,
};
use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};

fn create_network_from_source(src: &str) -> WorldNetwork {
    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).expect("loads");
    let linked = graph.link().expect("links");
    WorldNetwork::from_program(&linked).expect("creates network")
}

fn make_record_tuple(fields: &[(&str, &str)]) -> WorldTuple {
    let mut rec = TupleRecord::new();
    for (k, v) in fields {
        rec.set_str(*k, v);
    }
    rec.to_tuple()
}

#[test]
fn test_absent_key_insertion_and_oracle_differential() {
    let src = r#"
rel input order: { id: Str, customer: Str, sku: Str, qty: Int, status: Str } key id
rel input backorder: { sku: Str, available: Int } key sku

rel derived fulfillment =
    select { order_id: o.id, customer: o.customer, sku: o.sku, qty: o.qty }
    from o in order, b in backorder
    where o.sku == b.sku and b.available >= o.qty and o.status == "pending"

decide fulfill for f in fulfillment {
    propose fulfill_order priority 1 when f.qty > 0 = f.order_id
}
"#;

    let mut network = create_network_from_source(src);

    let batch = WorldBatch::new(
        0,
        "batch-1",
        vec![
            WorldBatchOp::Upsert {
                relation: "root::order".to_string(),
                key: WorldKey::from_str("ord-1"),
                tuple: make_record_tuple(&[
                    ("id", "ord-1"),
                    ("customer", "cust-A"),
                    ("sku", "widget-X"),
                    ("qty", "5"),
                    ("status", "pending"),
                ]),
            },
            WorldBatchOp::Upsert {
                relation: "root::backorder".to_string(),
                key: WorldKey::from_str("widget-X"),
                tuple: make_record_tuple(&[("sku", "widget-X"), ("available", "10")]),
            },
        ],
    );

    let report = network.apply_batch(&batch).expect("apply batch");
    assert_eq!(report.ops_applied, 2);

    let derived = network
        .get_derived_tuples("root::fulfillment")
        .expect("fulfillment tuples");
    assert_eq!(derived.len(), 1);
    assert_eq!(derived[0].get_str("order_id"), Some("ord-1"));
    assert_eq!(derived[0].get_str("customer"), Some("cust-A"));
    assert_eq!(derived[0].get_str("sku"), Some("widget-X"));
    assert_eq!(derived[0].get_str("qty"), Some("5"));

    let decision = network
        .get_settlement("root::fulfill", "ord-1")
        .expect("settled decision");
    assert_eq!(decision.candidate_name, "fulfill_order");
    assert_eq!(decision.value, Value::Str("ord-1".to_string()));

    network
        .verify_differential_correctness()
        .expect("oracle differential correctness");
}

#[test]
fn test_branch_switching_and_retraction() {
    let src = r#"
rel input order: { id: Str, customer: Str, sku: Str, qty: Int, status: Str } key id
rel input backorder: { sku: Str, available: Int } key sku

rel derived fulfillment =
    select { order_id: o.id, customer: o.customer, sku: o.sku, qty: o.qty }
    from o in order, b in backorder
    where o.sku == b.sku and b.available >= o.qty and o.status == "pending"

decide fulfill for f in fulfillment {
    propose fulfill_order priority 1 when f.qty > 0 = f.order_id
}
"#;

    let mut network = create_network_from_source(src);

    // Initial batch: order is pending
    let batch1 = WorldBatch::new(
        0,
        "batch-1",
        vec![
            WorldBatchOp::Upsert {
                relation: "root::order".to_string(),
                key: WorldKey::from_str("ord-1"),
                tuple: make_record_tuple(&[
                    ("id", "ord-1"),
                    ("customer", "cust-A"),
                    ("sku", "widget-X"),
                    ("qty", "5"),
                    ("status", "pending"),
                ]),
            },
            WorldBatchOp::Upsert {
                relation: "root::backorder".to_string(),
                key: WorldKey::from_str("widget-X"),
                tuple: make_record_tuple(&[("sku", "widget-X"), ("available", "10")]),
            },
        ],
    );
    network.apply_batch(&batch1).unwrap();
    assert_eq!(
        network
            .get_derived_tuples("root::fulfillment")
            .unwrap()
            .len(),
        1
    );
    network.verify_differential_correctness().unwrap();

    // Branch switch: update status to "cancelled", failing the filter predicate
    let batch2 = WorldBatch::new(
        1,
        "batch-2",
        vec![WorldBatchOp::Upsert {
            relation: "root::order".to_string(),
            key: WorldKey::from_str("ord-1"),
            tuple: make_record_tuple(&[
                ("id", "ord-1"),
                ("customer", "cust-A"),
                ("sku", "widget-X"),
                ("qty", "5"),
                ("status", "cancelled"),
            ]),
        }],
    );
    network.apply_batch(&batch2).unwrap();

    // Derived fulfillment and candidate must be retracted
    assert_eq!(
        network
            .get_derived_tuples("root::fulfillment")
            .unwrap()
            .len(),
        0
    );
    assert!(network.get_settlement("root::fulfill", "ord-1").is_none());
    network.verify_differential_correctness().unwrap();

    // Switch back to "pending"
    let batch3 = WorldBatch::new(
        2,
        "batch-3",
        vec![WorldBatchOp::Upsert {
            relation: "root::order".to_string(),
            key: WorldKey::from_str("ord-1"),
            tuple: make_record_tuple(&[
                ("id", "ord-1"),
                ("customer", "cust-A"),
                ("sku", "widget-X"),
                ("qty", "5"),
                ("status", "pending"),
            ]),
        }],
    );
    network.apply_batch(&batch3).unwrap();

    assert_eq!(
        network
            .get_derived_tuples("root::fulfillment")
            .unwrap()
            .len(),
        1
    );
    assert!(network.get_settlement("root::fulfill", "ord-1").is_some());
    network.verify_differential_correctness().unwrap();
}

#[test]
fn test_empty_to_nonempty_join() {
    let src = r#"
rel input order: { id: Str, sku: Str } key id
rel input stock: { sku: Str, count: Int } key sku

rel derived matching_stock =
    select { order_id: o.id, sku: o.sku }
    from o in order, s in stock
    where o.sku == s.sku
"#;

    let mut network = create_network_from_source(src);

    // 1. Insert order alone. Right side of join (stock) is empty.
    let batch1 = WorldBatch::new(
        0,
        "b1",
        vec![WorldBatchOp::Upsert {
            relation: "root::order".to_string(),
            key: WorldKey::from_str("ord-100"),
            tuple: make_record_tuple(&[("id", "ord-100"), ("sku", "sku-ABC")]),
        }],
    );
    network.apply_batch(&batch1).unwrap();
    assert_eq!(
        network
            .get_derived_tuples("root::matching_stock")
            .unwrap()
            .len(),
        0
    );
    network.verify_differential_correctness().unwrap();

    // 2. Insert stock for matching sku. Join transitions from empty to non-empty!
    let batch2 = WorldBatch::new(
        1,
        "b2",
        vec![WorldBatchOp::Upsert {
            relation: "root::stock".to_string(),
            key: WorldKey::from_str("sku-ABC"),
            tuple: make_record_tuple(&[("sku", "sku-ABC"), ("count", "50")]),
        }],
    );
    network.apply_batch(&batch2).unwrap();
    assert_eq!(
        network
            .get_derived_tuples("root::matching_stock")
            .unwrap()
            .len(),
        1
    );
    network.verify_differential_correctness().unwrap();

    // 3. Retract stock. Join transitions back to empty.
    let batch3 = WorldBatch::new(
        2,
        "b3",
        vec![WorldBatchOp::Remove {
            relation: "root::stock".to_string(),
            key: WorldKey::from_str("sku-ABC"),
        }],
    );
    network.apply_batch(&batch3).unwrap();
    assert_eq!(
        network
            .get_derived_tuples("root::matching_stock")
            .unwrap()
            .len(),
        0
    );
    network.verify_differential_correctness().unwrap();
}

#[test]
fn test_duplicate_derivations_and_distinct_set_semantics() {
    let src = r#"
rel input order: { id: Str, category: Str } key id

rel derived unique_categories =
    select { category: o.category }
    from o in order
"#;

    let mut network = create_network_from_source(src);

    // Insert first order in category "books"
    let batch1 = WorldBatch::new(
        0,
        "b1",
        vec![WorldBatchOp::Upsert {
            relation: "root::order".to_string(),
            key: WorldKey::from_str("ord-1"),
            tuple: make_record_tuple(&[("id", "ord-1"), ("category", "books")]),
        }],
    );
    let report1 = network.apply_batch(&batch1).unwrap();
    assert_eq!(report1.derived_tuples_inserted, 1);
    assert_eq!(
        network
            .get_derived_tuples("root::unique_categories")
            .unwrap()
            .len(),
        1
    );
    network.verify_differential_correctness().unwrap();

    // Insert second order in SAME category "books" (duplicate derivation)
    let batch2 = WorldBatch::new(
        1,
        "b2",
        vec![WorldBatchOp::Upsert {
            relation: "root::order".to_string(),
            key: WorldKey::from_str("ord-2"),
            tuple: make_record_tuple(&[("id", "ord-2"), ("category", "books")]),
        }],
    );
    let report2 = network.apply_batch(&batch2).unwrap();
    // Distinct set semantics: duplicate derivation does NOT emit an extra insert!
    assert_eq!(report2.derived_tuples_inserted, 0);
    assert_eq!(
        network
            .get_derived_tuples("root::unique_categories")
            .unwrap()
            .len(),
        1
    );
    network.verify_differential_correctness().unwrap();

    // Retract first order: category "books" survives because second support exists!
    let batch3 = WorldBatch::new(
        2,
        "b3",
        vec![WorldBatchOp::Remove {
            relation: "root::order".to_string(),
            key: WorldKey::from_str("ord-1"),
        }],
    );
    let report3 = network.apply_batch(&batch3).unwrap();
    assert_eq!(report3.derived_tuples_retracted, 0);
    assert_eq!(
        network
            .get_derived_tuples("root::unique_categories")
            .unwrap()
            .len(),
        1
    );
    network.verify_differential_correctness().unwrap();

    // Retract second order: last support removed! Distinct emits retraction.
    let batch4 = WorldBatch::new(
        3,
        "b4",
        vec![WorldBatchOp::Remove {
            relation: "root::order".to_string(),
            key: WorldKey::from_str("ord-2"),
        }],
    );
    let report4 = network.apply_batch(&batch4).unwrap();
    assert_eq!(report4.derived_tuples_retracted, 1);
    assert_eq!(
        network
            .get_derived_tuples("root::unique_categories")
            .unwrap()
            .len(),
        0
    );
    network.verify_differential_correctness().unwrap();
}

#[test]
fn test_last_support_removal_in_candidate_frontier() {
    let src = r#"
rel input alert: { id: Str, entity_id: Str, code: Str } key id

decide incident for a in alert {
    propose raise_ticket priority 1 when a.code == "ERR" = a.entity_id
}
"#;

    let mut network = create_network_from_source(src);

    // Two alerts with same code "ERR" for entity "srv-1"
    let batch1 = WorldBatch::new(
        0,
        "b1",
        vec![
            WorldBatchOp::Upsert {
                relation: "root::alert".to_string(),
                key: WorldKey::from_str("alt-1"),
                tuple: make_record_tuple(&[
                    ("id", "alt-1"),
                    ("entity_id", "srv-1"),
                    ("code", "ERR"),
                ]),
            },
            WorldBatchOp::Upsert {
                relation: "root::alert".to_string(),
                key: WorldKey::from_str("alt-2"),
                tuple: make_record_tuple(&[
                    ("id", "alt-2"),
                    ("entity_id", "srv-1"),
                    ("code", "ERR"),
                ]),
            },
        ],
    );
    network.apply_batch(&batch1).unwrap();

    // Candidate should have 2 derivation supports
    let candidates = network
        .get_candidates("root::incident", "srv-1")
        .expect("candidates for srv-1");
    let entry = &candidates["raise_ticket"];
    assert_eq!(entry.supports.len(), 2);
    assert_eq!(
        network
            .get_settlement("root::incident", "srv-1")
            .unwrap()
            .candidate_name,
        "raise_ticket"
    );
    network.verify_differential_correctness().unwrap();

    // Retract first alert: candidate survives with 1 remaining support
    let batch2 = WorldBatch::new(
        1,
        "b2",
        vec![WorldBatchOp::Remove {
            relation: "root::alert".to_string(),
            key: WorldKey::from_str("alt-1"),
        }],
    );
    network.apply_batch(&batch2).unwrap();
    let candidates_after_one = network
        .get_candidates("root::incident", "srv-1")
        .expect("candidates still exist");
    assert_eq!(candidates_after_one["raise_ticket"].supports.len(), 1);
    assert!(network.get_settlement("root::incident", "srv-1").is_some());
    network.verify_differential_correctness().unwrap();

    // Retract second alert: last support removed, candidate retracted!
    let batch3 = WorldBatch::new(
        2,
        "b3",
        vec![WorldBatchOp::Remove {
            relation: "root::alert".to_string(),
            key: WorldKey::from_str("alt-2"),
        }],
    );
    network.apply_batch(&batch3).unwrap();
    assert!(network.get_candidates("root::incident", "srv-1").is_none());
    assert!(network.get_settlement("root::incident", "srv-1").is_none());
    network.verify_differential_correctness().unwrap();
}

#[test]
fn test_multi_hop_retractions() {
    let src = r#"
rel input parent_item: { id: Str, group: Str } key id
rel input child_item: { id: Str, parent_id: Str, active: Bool } key id

rel derived active_children =
    select { id: c.id, parent_id: c.parent_id }
    from c in child_item
    where c.active == true

rel derived linked_parent_child =
    select { child_id: c.id, parent_id: p.id, group: p.group }
    from p in parent_item, c in active_children
    where p.id == c.parent_id

decide group_decision for l in linked_parent_child {
    propose handle_item priority 1 when true = l.child_id
}
"#;

    let mut network = create_network_from_source(src);

    let batch1 = WorldBatch::new(
        0,
        "b1",
        vec![
            WorldBatchOp::Upsert {
                relation: "root::parent_item".to_string(),
                key: WorldKey::from_str("p-1"),
                tuple: make_record_tuple(&[("id", "p-1"), ("group", "G1")]),
            },
            WorldBatchOp::Upsert {
                relation: "root::child_item".to_string(),
                key: WorldKey::from_str("c-1"),
                tuple: make_record_tuple(&[
                    ("id", "c-1"),
                    ("parent_id", "p-1"),
                    ("active", "true"),
                ]),
            },
        ],
    );
    network.apply_batch(&batch1).unwrap();

    assert_eq!(
        network
            .get_derived_tuples("root::active_children")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        network
            .get_derived_tuples("root::linked_parent_child")
            .unwrap()
            .len(),
        1
    );
    assert!(network
        .get_settlement("root::group_decision", "c-1")
        .is_some());
    network.verify_differential_correctness().unwrap();

    // Multi-hop retraction: remove parent_item at the root of the dependency chain
    let batch2 = WorldBatch::new(
        1,
        "b2",
        vec![WorldBatchOp::Remove {
            relation: "root::parent_item".to_string(),
            key: WorldKey::from_str("p-1"),
        }],
    );
    network.apply_batch(&batch2).unwrap();

    // child_item is still active, but linked_parent_child and decision must be retracted!
    assert_eq!(
        network
            .get_derived_tuples("root::active_children")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        network
            .get_derived_tuples("root::linked_parent_child")
            .unwrap()
            .len(),
        0
    );
    assert!(network
        .get_settlement("root::group_decision", "c-1")
        .is_none());
    network.verify_differential_correctness().unwrap();
}

#[test]
fn test_grouped_count_state_transitions() {
    let src = r#"
rel input item: { id: Str, category: Str } key id

rel derived category_count =
    select { category: i.category, cnt: count() }
    from i in item
    group by i.category
"#;

    let mut network = create_network_from_source(src);

    // 1. Transition 0 -> 1 on first insert in category "hardware"
    let batch1 = WorldBatch::new(
        0,
        "b1",
        vec![WorldBatchOp::Upsert {
            relation: "root::item".to_string(),
            key: WorldKey::from_str("item-1"),
            tuple: make_record_tuple(&[("id", "item-1"), ("category", "hardware")]),
        }],
    );
    network.apply_batch(&batch1).unwrap();
    let tuples1 = network.get_derived_tuples("root::category_count").unwrap();
    assert_eq!(tuples1.len(), 1);
    assert_eq!(tuples1[0].get_str("category"), Some("hardware"));
    assert_eq!(tuples1[0].get_str("cnt"), Some("1"));
    network.verify_differential_correctness().unwrap();

    // 2. Count update 1 -> 2 on second insert in category "hardware"
    let batch2 = WorldBatch::new(
        1,
        "b2",
        vec![WorldBatchOp::Upsert {
            relation: "root::item".to_string(),
            key: WorldKey::from_str("item-2"),
            tuple: make_record_tuple(&[("id", "item-2"), ("category", "hardware")]),
        }],
    );
    network.apply_batch(&batch2).unwrap();
    let tuples2 = network.get_derived_tuples("root::category_count").unwrap();
    assert_eq!(tuples2.len(), 1);
    assert_eq!(tuples2[0].get_str("category"), Some("hardware"));
    assert_eq!(tuples2[0].get_str("cnt"), Some("2"));
    network.verify_differential_correctness().unwrap();

    // 3. Count update 2 -> 3 on third insert
    let batch3 = WorldBatch::new(
        2,
        "b3",
        vec![WorldBatchOp::Upsert {
            relation: "root::item".to_string(),
            key: WorldKey::from_str("item-3"),
            tuple: make_record_tuple(&[("id", "item-3"), ("category", "hardware")]),
        }],
    );
    network.apply_batch(&batch3).unwrap();
    let tuples3 = network.get_derived_tuples("root::category_count").unwrap();
    assert_eq!(tuples3.len(), 1);
    assert_eq!(tuples3[0].get_str("cnt"), Some("3"));
    network.verify_differential_correctness().unwrap();

    // 4. Count update on retract 3 -> 2
    let batch4 = WorldBatch::new(
        3,
        "b4",
        vec![WorldBatchOp::Remove {
            relation: "root::item".to_string(),
            key: WorldKey::from_str("item-2"),
        }],
    );
    network.apply_batch(&batch4).unwrap();
    let tuples4 = network.get_derived_tuples("root::category_count").unwrap();
    assert_eq!(tuples4.len(), 1);
    assert_eq!(tuples4[0].get_str("cnt"), Some("2"));
    network.verify_differential_correctness().unwrap();

    // 5. Retract down to 1 -> 0
    let batch5 = WorldBatch::new(
        4,
        "b5",
        vec![
            WorldBatchOp::Remove {
                relation: "root::item".to_string(),
                key: WorldKey::from_str("item-1"),
            },
            WorldBatchOp::Remove {
                relation: "root::item".to_string(),
                key: WorldKey::from_str("item-3"),
            },
        ],
    );
    network.apply_batch(&batch5).unwrap();
    let tuples5 = network.get_derived_tuples("root::category_count").unwrap();
    assert_eq!(tuples5.len(), 0);
    network.verify_differential_correctness().unwrap();
}

#[test]
fn test_canonical_settlement_priority_and_tiebreak() {
    let src = r#"
rel input task: { id: Str, tier: Str } key id

decide schedule for t in task {
    propose urgent priority 1 when t.tier == "P1" = "urgent_handling"
    propose standard priority 5 when true = "standard_handling"
}
"#;

    let mut network = create_network_from_source(src);

    // Task with tier P1 matches both proposals: urgent (priority 1) and standard (priority 5)
    let batch1 = WorldBatch::new(
        0,
        "b1",
        vec![WorldBatchOp::Upsert {
            relation: "root::task".to_string(),
            key: WorldKey::from_str("task-42"),
            tuple: make_record_tuple(&[("id", "task-42"), ("tier", "P1")]),
        }],
    );
    network.apply_batch(&batch1).unwrap();

    // Smaller priority = more urgent -> urgent (priority 1) must be settled
    let decision1 = network.get_settlement("root::schedule", "task-42").unwrap();
    assert_eq!(decision1.candidate_name, "urgent");
    assert_eq!(decision1.priority, 1);
    assert_eq!(decision1.value, Value::Str("urgent_handling".into()));
    network.verify_differential_correctness().unwrap();

    // Downgrade task to P2: urgent guard fails, only standard (priority 5) remains
    let batch2 = WorldBatch::new(
        1,
        "b2",
        vec![WorldBatchOp::Upsert {
            relation: "root::task".to_string(),
            key: WorldKey::from_str("task-42"),
            tuple: make_record_tuple(&[("id", "task-42"), ("tier", "P2")]),
        }],
    );
    network.apply_batch(&batch2).unwrap();

    let decision2 = network.get_settlement("root::schedule", "task-42").unwrap();
    assert_eq!(decision2.candidate_name, "standard");
    assert_eq!(decision2.priority, 5);
    assert_eq!(decision2.value, Value::Str("standard_handling".into()));
    network.verify_differential_correctness().unwrap();
}

#[test]
fn test_pure_scalar_helper_function_evaluation() {
    let src = r#"
fn format_id(x: Str): Str = x

rel input row: { id: Str } key id

rel derived formatted =
    select { id: format_id(r.id) }
    from r in row
"#;

    let mut network = create_network_from_source(src);

    let batch = WorldBatch::new(
        0,
        "b1",
        vec![WorldBatchOp::Upsert {
            relation: "root::row".to_string(),
            key: WorldKey::from_str("R-99"),
            tuple: make_record_tuple(&[("id", "R-99")]),
        }],
    );
    network.apply_batch(&batch).unwrap();

    let tuples = network.get_derived_tuples("root::formatted").unwrap();
    assert_eq!(tuples.len(), 1);
    assert_eq!(tuples[0].get_str("id"), Some("R-99"));
    network.verify_differential_correctness().unwrap();
}
