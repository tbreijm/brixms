//! Unit and qualification tests for relational DAG lowering and profile constraints (ADR-0046 P2).

use std::collections::BTreeMap;

use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};
use brix_lower::relation_dag::{
    admit_named_queries_v1, lower_named_query_v1, lower_relations, OperatorNode,
    RelationalLowerError,
};

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
fn named_query_admission_resolves_schema_and_rejects_unsupported_param_type() {
    let source = r#"
rel input catalogue_entries: { id: Str, kind: Str } key id
query catalogue(kind: Str) = select { id: c.id, kind: c.kind }
  from c in catalogue_entries where c.kind == kind
"#;
    let sources = BTreeMap::from([("root".to_string(), source.to_string())]);
    let loader = |name: &str| sources.get(name).cloned();
    let linked = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .unwrap()
        .link()
        .unwrap();
    let schema = admit_named_queries_v1(&linked).unwrap();
    assert_eq!(schema["catalogue"].result_fields.len(), 2);
    assert_eq!(schema["catalogue"].params[0].name, "kind");

    let bad = source.replace("kind: Str", "kind: Float");
    let sources = BTreeMap::from([("root".to_string(), bad)]);
    let loader = |name: &str| sources.get(name).cloned();
    let linked = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .unwrap()
        .link()
        .unwrap();
    assert!(admit_named_queries_v1(&linked).is_err());
}

#[test]
fn named_query_lowering_prunes_unrelated_relations_and_decisions() {
    let source = r#"
rel input catalogue_entries: { id: Str } key id
rel input unrelated_rows: { id: Str } key id
rel derived unused_branch = select { id: u.id } from u in unrelated_rows
decide unrelated_decision for u in unrelated_rows per id {
  propose keep priority 1 when true = "yes"
}
query catalogue() = select { id: c.id } from c in catalogue_entries
"#;
    let sources = BTreeMap::from([("root".to_string(), source.to_string())]);
    let loader = |name: &str| sources.get(name).cloned();
    let linked = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .unwrap()
        .link()
        .unwrap();
    let query = linked
        .root_items
        .iter()
        .find_map(|item| match item {
            brix_syntax::ast::Item::Query(query) => Some(query),
            _ => None,
        })
        .unwrap();
    let lowered = lower_named_query_v1(&linked, query, &BTreeMap::new()).unwrap();
    assert_eq!(lowered.program.rel_inputs.len(), 1);
    assert!(lowered
        .program
        .rel_inputs
        .keys()
        .all(|name| name.name == "catalogue_entries"));
    assert_eq!(lowered.program.rel_derived.len(), 1);
    assert!(lowered.program.decides.is_empty());
    assert!(lowered
        .dag
        .relation_outputs
        .keys()
        .all(|name| !name.contains("unrelated") && !name.contains("unused_branch")));
}

#[test]
fn decision_winner_sources_share_one_acyclic_lowering_order() {
    let src = r#"
rel input orders: { id: Str, quote: Int } key id
decide approve for o in orders per id { propose accept priority 1 when true = o.quote }
rel derived approved = select { id: a.id, quote: a.value } from a in approve
decide dispatch for a in approved per id { propose ship priority 2 when true = a.quote }
"#;
    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();
    let linked = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .expect("loads")
        .link()
        .expect("links");
    let dag = lower_relations(&linked).expect("lowering");

    let approve = dag.relation_outputs["root::approve"];
    let approved = dag.relation_outputs["root::approved"];
    let dispatch = dag.relation_outputs["root::dispatch"];
    assert!(
        approve < approved && approved < dispatch,
        "stages must be topologically ordered"
    );
    assert!(matches!(
        dag.nodes[approve.0],
        OperatorNode::DecisionRows { .. }
    ));
    assert!(matches!(
        dag.nodes[dispatch.0],
        OperatorNode::DecisionRows { .. }
    ));
    let schemas = brix_lower::relation_dag::decision_field_schemas(&dag, &linked);
    assert_eq!(
        schemas[approve.0]["value"],
        brix_syntax::ast::Ty::Named("Int".into())
    );
}

#[test]
fn correlated_not_exists_lowers_to_indexed_anti_join_and_refuses_cycles() {
    let src = r#"
rel input orders: { id: Str } key id
rel input quote: { order_id: Str } key order_id
rel derived unquoted = select { id: o.id } from o in orders
    where not exists q in quote where q.order_id == o.id
"#;
    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();
    let linked = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .expect("loads")
        .link()
        .expect("links");
    let dag = lower_relations(&linked).expect("lowers anti-join");
    assert!(dag.nodes.iter().any(|node| matches!(node,
        OperatorNode::AntiJoin { left_keys, right_keys, .. }
            if left_keys == &[brix_lower::relation_dag::FieldRef { binding: "o".into(), field: "id".into() }]
                && right_keys == &[brix_lower::relation_dag::FieldRef { binding: "q".into(), field: "order_id".into() }]
    )));

    let cyclic = r#"
rel input seed: { id: Str } key id
rel derived a = select { id: s.id } from s in seed where not exists q in b where q.id == s.id
rel derived b = select { id: x.id } from x in a
"#;
    sources.insert("root".to_string(), cyclic.to_string());
    let linked = ModuleGraph::load(
        "root",
        &|name: &str| sources.get(name).cloned(),
        ModuleLoaderLimits::default(),
    )
    .expect("loads")
    .link()
    .expect("links");
    assert!(matches!(
        lower_relations(&linked),
        Err(RelationalLowerError::UnstratifiedNegation { .. })
    ));
}

#[test]
fn absence_and_zero_count_refuse_ambiguous_or_lossy_query_shapes() {
    let cases = [
        (
            "rel input parents: { id: Str } key id\nrel input children: { id: Str } key id",
            "select { id: p.id } from p in parents where not exists p in children where p.id == p.id",
            "duplicates an outer query binding",
        ),
        (
            "rel input parents: { id: Str, tenant: Str } key (id, tenant)\nrel input children: { id: Str } key id",
            "select { id: p.id, count: count_or_zero() } from p in parents, c in children where p.id == c.id group by p.id",
            "complete single-field key",
        ),
        (
            "rel input parents: { id: Str } key id\nrel input children: { id: Str } key id",
            "select { id: p.id, count: count_or_zero(), also: count_or_zero() } from p in parents, c in children where p.id == c.id group by p.id",
            "exactly two distinctly named projections",
        ),
        (
            "rel input parents: { id: Str } key id\nrel input children: { id: Str } key id",
            "select { id: p.id, also: p.id, count: count_or_zero() } from p in parents, c in children where p.id == c.id group by p.id",
            "exactly two distinctly named projections",
        ),
        (
            "rel input parents: { id: Str } key id\nrel input children: { id: Str } key id",
            "select { id: p.id, id: count_or_zero() } from p in parents, c in children where p.id == c.id group by p.id",
            "exactly two distinctly named projections",
        ),
    ];
    for (declarations, query, diagnostic) in cases {
        let source = format!("{declarations}\nrel derived result = {query}");
        let linked = ModuleGraph::load(
            "root",
            &|_: &str| Some(source.clone()),
            ModuleLoaderLimits::default(),
        )
        .expect("parse and load")
        .link()
        .expect("link");
        let error = lower_relations(&linked).expect_err("must refuse before execution");
        assert!(
            matches!(&error, RelationalLowerError::UnsupportedExpression(message)
                        if message.contains(diagnostic)),
            "{query}: {error:?}"
        );
    }
}

#[test]
fn relation_decide_dependency_cycle_is_refused_with_participants() {
    let src = r#"
rel input orders: { id: Str, quote: Int } key id
rel derived loop = select { id: a.id } from a in approve
decide approve for x in loop per id { propose accept priority 1 when true = 1 }
"#;
    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();
    let linked = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .expect("loads")
        .link()
        .expect("links");
    let err = lower_relations(&linked).expect_err("decision/relation cycle is invalid");
    match err {
        RelationalLowerError::RecursiveRelationCycle { cycle } => {
            assert!(cycle.contains(&"root::approve".to_string()));
            assert!(cycle.contains(&"root::loop".to_string()));
        }
        other => panic!("expected a named dependency cycle, got {other:?}"),
    }
}

#[test]
fn winner_row_schema_rejects_conflicting_values_reserved_keys_and_int_overflow() {
    let cases = [
        (
            "rel input rows: { id: Str } key id\ndecide d for r in rows per id { propose a priority 1 when true = 1 propose b priority 2 when true = \"x\" }",
            "inconsistent result types",
        ),
        (
            "rel input rows: { candidate: Str } key candidate\ndecide d for r in rows per candidate { propose a priority 1 when true = 1 }",
            "reserved winner-row field",
        ),
        (
            "rel input rows: { id: Str } key id\ndecide d for r in rows per id { propose a priority 18446744073709551615 when true = 1 }",
            "winner-row Int range",
        ),
    ];
    for (source, expected) in cases {
        let sources = BTreeMap::from([("root".to_string(), source.to_string())]);
        let loader = |name: &str| sources.get(name).cloned();
        let linked = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
            .expect("loads")
            .link()
            .expect("links");
        let error = lower_relations(&linked).expect_err("invalid winner row contract");
        assert!(error.to_string().contains(expected), "{error}");
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
fn bounded_collect_take_has_distinct_dag_projection_and_record_schema() {
    let source = r#"
rel input events: { id: Str, category: Str, value: Str } key id
rel derived grouped =
    select { category: e.category, recent: collect(e.value, e.id).take(2) }
    from e in events
    group by e.category
"#;
    let sources = BTreeMap::from([("root".to_string(), source.to_string())]);
    let loader = |name: &str| sources.get(name).cloned();
    let linked = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .unwrap()
        .link()
        .unwrap();
    let dag = lower_relations(&linked).expect("bounded collect should lower");
    let grouped = dag
        .nodes
        .iter()
        .enumerate()
        .find_map(|(index, node)| match node {
            OperatorNode::GroupedCount { projections, .. } => Some((index, projections)),
            _ => None,
        })
        .expect("grouped projection node");
    assert!(grouped.1.iter().any(|(name, projection)| {
        name == "recent"
            && matches!(
                projection,
                brix_lower::relation_dag::GroupProjection::CollectTake {
                    limit: 2,
                    order_by: Some(_),
                    ..
                }
            )
    }));
    let schemas = brix_lower::relation_dag::decision_field_schemas(&dag, &linked);
    let recent = schemas[grouped.0].get("recent").expect("recent schema");
    assert_eq!(
        recent,
        &brix_syntax::ast::Ty::Record(vec![
            brix_syntax::ast::FieldDecl {
                name: "items".into(),
                ty: brix_syntax::ast::Ty::App(
                    "List".into(),
                    vec![brix_syntax::ast::Ty::Named("Str".into())],
                ),
            },
            brix_syntax::ast::FieldDecl {
                name: "truncated".into(),
                ty: brix_syntax::ast::Ty::Named("Bool".into()),
            },
        ])
    );
}

#[test]
fn bounded_collect_take_accepts_zero_and_rejects_invalid_limits() {
    let base = r#"
rel input events: { id: Str, category: Str, value: Str } key id
rel derived grouped =
    select { category: e.category, recent: collect(e.value).take(LIMIT) }
    from e in events
    group by e.category
"#;
    for (limit, accepted) in [
        ("0", true),
        ("1", true),
        ("-1", false),
        ("1.5", false),
        ("n", false),
        ("65537", false),
    ] {
        let source = base.replace("LIMIT", limit);
        let sources = BTreeMap::from([("root".to_string(), source)]);
        let loader = |name: &str| sources.get(name).cloned();
        let linked = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
            .unwrap()
            .link()
            .unwrap();
        assert_eq!(lower_relations(&linked).is_ok(), accepted, "limit {limit}");
    }
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

#[test]
fn test_scalar_projection_accepts_expression_and_exposes_value_field() {
    let src = r#"
rel input rows: { id: Str, count: Int } key id
rel derived ids = select r.id from r in rows
"#;
    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).expect("loads");
    let linked = graph.link().expect("links");
    let dag = lower_relations(&linked).expect("scalar projection must be accepted");

    let ids_op = dag.relation_outputs["root::ids"];
    let distinct_node = &dag.nodes[ids_op.0];
    let project_op = match distinct_node {
        OperatorNode::Distinct { input } => *input,
        other => panic!("expected Distinct, got {other:?}"),
    };
    let node = &dag.nodes[project_op.0];
    match node {
        OperatorNode::Project { projections, .. } => {
            assert_eq!(projections.len(), 1);
            assert_eq!(projections[0].0, "value");
        }
        other => panic!("expected Project, got {other:?}"),
    }
}

#[test]
fn test_empty_anon_record_projection_rejected() {
    let src = r#"
rel input rows: { id: Str } key id
rel derived bad = select {} from r in rows
"#;
    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).expect("loads");
    let linked = graph.link().expect("links");
    let err = lower_relations(&linked).expect_err("empty anon record must be rejected");
    match err {
        RelationalLowerError::UnsupportedExpression(msg) => {
            assert!(
                msg.contains("must not be an empty record projection"),
                "{msg}"
            );
        }
        other => panic!("expected UnsupportedExpression, got {other:?}"),
    }
}

#[test]
fn test_union_schema_resolved_through_dag_without_value_fallback() {
    let src = r#"
rel input r1: { id: Str, tag: Str, count: Int } key id
rel input r2: { id: Str, tag: Str, count: Int } key id
rel derived u = union r1, r2 key id
rel derived projected = select { id: u.id, tag: u.tag, twice: u.count } from u in u
"#;
    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).expect("loads");
    let linked = graph.link().expect("links");
    let dag = lower_relations(&linked).expect("union schema resolution must succeed");

    assert!(dag.relation_outputs.contains_key("root::u"));
    assert!(dag.relation_outputs.contains_key("root::projected"));
}

#[test]
fn test_union_distinct_lowering_to_operator_node() {
    let src = r#"
rel input a: { id: Str, name: Str } key id
rel input b: { id: Str, name: Str } key id
rel input c: { id: Str, name: Str } key id
rel derived combined = union distinct a, b, c
"#;
    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).expect("loads");
    let linked = graph.link().expect("links");
    let dag = lower_relations(&linked).expect("union distinct lowering");

    let combined_op = dag.relation_outputs["root::combined"];
    match &dag.nodes[combined_op.0] {
        OperatorNode::UnionDistinct { sources } => {
            assert_eq!(sources.len(), 3);
        }
        other => panic!("expected UnionDistinct node, got {other:?}"),
    }
}

#[test]
fn test_union_mismatched_schemas_rejected_with_diagnostic() {
    let src = r#"
rel input r1: { id: Str, amount: Int } key id
rel input r2: { id: Str, amount: Str } key id
rel derived bad_u = union r1, r2 key id
"#;
    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).expect("loads");
    let linked = graph.link().expect("links");
    let err = lower_relations(&linked).expect_err("mismatched union schemas must fail");
    match err {
        RelationalLowerError::UnsupportedExpression(msg) => {
            assert!(msg.contains("bad_u"), "{msg}");
            assert!(msg.contains("r2"), "{msg}");
            assert!(msg.contains("amount"), "{msg}");
        }
        other => panic!("expected UnsupportedExpression, got {other:?}"),
    }
}

#[test]
fn test_composite_decision_key_lowering_and_winner_schema() {
    let src = r#"
rel input tenants: { tenant: Str, id: Str, quota: Int } key id
decide allocate for t in tenants per (tenant, id) {
    propose grant priority 1 when t.quota > 0 = t.quota
}
rel derived granted = select { tenant: a.tenant, id: a.id, quota: a.value } from a in allocate
"#;
    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).expect("loads");
    let linked = graph.link().expect("links");
    let dag = lower_relations(&linked).expect("composite decision key lowering");

    assert!(dag.relation_outputs.contains_key("root::allocate"));
    assert!(dag.relation_outputs.contains_key("root::granted"));
}

#[test]
fn test_nested_field_path_equijoin_lowering() {
    let src = r#"
config Meta = { score: Int, label: Str }
config Wrapper = { meta: Meta, owner: Str }

rel input accounts: { id: Str, w: Wrapper } key id
rel input targets: { id: Str, target_score: Int } key id

rel derived matched = select { account: a.id, target: b.id }
    from a in accounts, b in targets
    where a.w.meta.score == b.target_score
"#;
    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).expect("loads");
    let linked = graph.link().expect("links");
    let dag = lower_relations(&linked).expect("nested field path equijoin lowering");

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
            assert_eq!(left_keys[0].binding, "a");
            assert_eq!(left_keys[0].field, "w.meta.score");
            assert_eq!(right_keys[0].binding, "b");
            assert_eq!(right_keys[0].field, "target_score");
        }
        other => panic!("expected EquiJoin, got {other:?}"),
    }
}

#[test]
fn test_nested_field_path_through_unnarrowed_variant_rejected() {
    let src = r#"
config Shape = Circle(Int) | Rect(Int, Int)
config Wrapper = { shape: Shape }

rel input items: { id: Str, w: Wrapper } key id
rel input targets: { id: Str, radius: Int } key id

rel derived bad_join = select { item: i.id }
    from i in items, t in targets
    where i.w.shape.radius == t.radius
"#;
    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();
    let load_res = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default());
    match load_res {
        Err(err) => {
            let msg = format!("{err:?}");
            assert!(
                msg.contains("cannot project field") || msg.contains("through sum/variant type"),
                "{msg}"
            );
        }
        Ok(graph) => {
            let linked = graph.link().expect("links");
            let err = lower_relations(&linked)
                .expect_err("path through un-narrowed sum variant must fail");
            let msg = format!("{err:?}");
            assert!(
                msg.contains("through sum") || msg.contains("without pattern matching"),
                "{msg}"
            );
        }
    }
}
