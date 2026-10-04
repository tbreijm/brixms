//! Unit and qualification tests for relational DAG lowering and profile constraints (ADR-0046 P2).

use std::collections::BTreeMap;

use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};
use brix_lower::relation_dag::{lower_relations, OperatorNode, RelationalLowerError};

#[test]
fn test_adr0046_fulfillment_lowering_to_operator_dag() {
    let src = r#"
rel input order: { id: Str, customer: Str, sku: Str, qty: Int, status: Str } key id
rel input backorder: { sku: Str, available: Int } key sku

rel derived fulfillment =
    select { order_id: o.id, customer: o.customer, sku: o.sku, qty: o.qty }
    from o in order, b in backorder
    where o.sku == b.sku and b.available >= o.qty and o.status == "pending"
"#;

    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();

    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).expect("loads");
    let linked = graph.link().expect("links");
    let dag = lower_relations(&linked).expect("lowering to operator DAG must succeed");

    assert!(dag.relation_outputs.contains_key("root::order"));
    assert!(dag.relation_outputs.contains_key("root::backorder"));
    assert!(dag.relation_outputs.contains_key("root::fulfillment"));

    // Verify all admitted operators are present in DAG
    let has_scan = dag
        .nodes
        .iter()
        .any(|n| matches!(n, OperatorNode::Scan { .. }));
    let has_equijoin = dag
        .nodes
        .iter()
        .any(|n| matches!(n, OperatorNode::EquiJoin { .. }));
    let has_filter = dag
        .nodes
        .iter()
        .any(|n| matches!(n, OperatorNode::Filter { .. }));
    let has_project = dag
        .nodes
        .iter()
        .any(|n| matches!(n, OperatorNode::Project { .. }));
    let has_distinct = dag
        .nodes
        .iter()
        .any(|n| matches!(n, OperatorNode::Distinct { .. }));

    assert!(has_scan, "DAG must contain Scan");
    assert!(has_equijoin, "DAG must contain EquiJoin");
    assert!(has_filter, "DAG must contain Filter");
    assert!(has_project, "DAG must contain Project");
    assert!(has_distinct, "DAG must contain Distinct");

    // Check equijoin keys
    let join_node = dag
        .nodes
        .iter()
        .find(|n| matches!(n, OperatorNode::EquiJoin { .. }))
        .expect("join node exists");
    match join_node {
        OperatorNode::EquiJoin {
            left_keys,
            right_keys,
            ..
        } => {
            assert_eq!(left_keys[0].binding, "o");
            assert_eq!(left_keys[0].field, "sku");
            assert_eq!(right_keys[0].binding, "b");
            assert_eq!(right_keys[0].field, "sku");
        }
        _ => unreachable!(),
    }
}

#[test]
fn test_grouped_count_lowering_to_operator_dag() {
    let src = r#"
rel input order: { id: Str, sku: Str, qty: Int } key id
rel input backorder: { sku: Str, available: Int } key sku

rel derived backordered_sku_count =
    select { sku: b.sku, pending_orders: count() }
    from o in order, b in backorder
    where o.sku == b.sku and b.available < o.qty
    group by b.sku
"#;

    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();

    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).expect("loads");
    let linked = graph.link().expect("links");
    let dag = lower_relations(&linked).expect("lowering to operator DAG must succeed");

    let has_grouped_count = dag.nodes.iter().any(|n| match n {
        OperatorNode::GroupedCount { projections, .. } => {
            projections.iter().any(|(name, projection)| {
                name == "pending_orders"
                    && *projection == brix_lower::relation_dag::GroupProjection::Count
            })
        }
        _ => false,
    });
    assert!(
        has_grouped_count,
        "DAG must contain GroupedCount operator with pending_orders alias"
    );
}

#[test]
fn test_recursive_relation_cycle_rejected() {
    let src = r#"
rel input base: { id: Str } key id

rel derived r1 =
    select { id: r2.id }
    from r2 in r2

rel derived r2 =
    select { id: r1.id }
    from r1 in r1
"#;

    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();

    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).expect("loads");
    let linked = graph.link().expect("links");
    let err = lower_relations(&linked).expect_err("recursive relation cycle must be rejected");

    match err {
        RelationalLowerError::RecursiveRelationCycle { cycle } => {
            assert!(cycle.contains(&"root::r1".to_string()));
            assert!(cycle.contains(&"root::r2".to_string()));
        }
        other => panic!("expected RecursiveRelationCycle, got {other:?}"),
    }
}

#[test]
fn test_self_recursive_relation_rejected() {
    let src = r#"
rel derived r =
    select { id: self_r.id }
    from self_r in r
"#;

    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();

    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).expect("loads");
    let linked = graph.link().expect("links");
    let err = lower_relations(&linked).expect_err("self-recursive relation must be rejected");

    match err {
        RelationalLowerError::RecursiveRelationCycle { cycle } => {
            assert_eq!(cycle, vec!["root::r".to_string(), "root::r".to_string()]);
        }
        other => panic!("expected RecursiveRelationCycle, got {other:?}"),
    }
}

#[test]
fn test_unstratified_negation_rejected() {
    // True unstratified negation: active_orders negates pending_orders,
    // while pending_orders depends on active_orders (cyclic dependency with negation).
    let src = r#"
rel input order: { id: Str } key id

rel derived pending_orders =
    select { id: o.id }
    from o in order, a in active_orders

rel derived active_orders =
    select { id: o.id }
    from o in order
    where !pending_orders
"#;

    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();

    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).expect("loads");
    let linked = graph.link().expect("links");
    let err = lower_relations(&linked).expect_err("unstratified negation must be rejected");

    match err {
        RelationalLowerError::UnstratifiedNegation {
            relation,
            negated_target,
        } => {
            assert_eq!(relation, "root::active_orders");
            assert_eq!(negated_target, "pending_orders");
        }
        other => panic!("expected UnstratifiedNegation, got {other:?}"),
    }
}

#[test]
fn test_unsupported_relational_negation_rejected() {
    // Relational negation outside cyclic SCC (e.g. negating an input relation)
    // is rejected as an unsupported profile operator in brix.world@1.
    let src = r#"
rel input order: { id: Str } key id
rel input cancel: { id: Str } key id

rel derived active_orders =
    select { id: o.id }
    from o in order
    where !cancel
"#;

    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();

    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).expect("loads");
    let linked = graph.link().expect("links");
    let err =
        lower_relations(&linked).expect_err("unsupported relational negation must be rejected");

    match err {
        RelationalLowerError::UnsupportedNegation { relation, target } => {
            assert_eq!(relation, "root::active_orders");
            assert_eq!(target, "cancel");
        }
        other => panic!("expected UnsupportedNegation, got {other:?}"),
    }
}

#[test]
fn test_missing_equijoin_predicate_rejected() {
    let src = r#"
rel input r1: { id: Str, a: Int } key id
rel input r2: { id: Str, b: Int } key id

rel derived bad_join =
    select { id: o1.id, val: o2.b }
    from o1 in r1, o2 in r2
    where o1.a > o2.b
"#;

    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();

    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).expect("loads");
    let linked = graph.link().expect("links");
    let err = lower_relations(&linked).expect_err("cartesian product without equijoin must fail");

    match err {
        RelationalLowerError::MissingEquiJoinPredicate { .. } => {}
        other => panic!("expected MissingEquiJoinPredicate, got {other:?}"),
    }
}
