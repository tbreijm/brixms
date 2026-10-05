//! Executable-contract regressions: inspect complete operator wiring, not just node kinds.
use brix_lower::module_graph::{ModuleGraph, ModuleLinkError, ModuleLoaderLimits, SizedLoader};
use brix_lower::relation_dag::{lower_relations, GroupProjection, OperatorNode, RelationDag};
use std::collections::BTreeMap;

fn graph(sources: &[(&str, &str)]) -> ModuleGraph {
    let sources: BTreeMap<_, _> = sources
        .iter()
        .map(|(name, source)| (*name, source.to_string()))
        .collect();
    ModuleGraph::load(
        "root",
        &|name: &str| sources.get(name).cloned(),
        ModuleLoaderLimits::default(),
    )
    .unwrap()
}
fn dag(source: &str) -> Result<RelationDag, String> {
    let program = graph(&[("root", source)])
        .link()
        .map_err(|e| e.to_string())?;
    lower_relations(&program).map_err(|e| e.to_string())
}

#[test]
fn imported_derived_relation_uses_its_own_private_source_and_helper() {
    let program = graph(&[
        ("root", "use lib\nrel input rows: { id: Str } key id\nrel derived chosen = select { id: x.id } from x in lib::visible"),
        ("lib", "rel input rows: { id: Str } key id\nfn keep(x: Str): Str = x\nexport rel derived visible = select { id: keep(x.id) } from x in rows"),
    ]).link().unwrap();
    let dag = lower_relations(&program).unwrap();
    let lib_scan = dag.relation_outputs["lib::rows"];
    assert!(dag.nodes.iter().any(|node| matches!(node, OperatorNode::Bind { input, alias } if *input == lib_scan && alias == "x")));
    assert!(dag.nodes.iter().any(|node| matches!(node, OperatorNode::Project { projections, .. }
        if matches!(&projections[0].1, brix_syntax::ast::Expr::Call { func, .. } if func == "lib::keep"))));
}

#[test]
fn unqualified_names_never_select_an_arbitrary_foreign_suffix() {
    let program = graph(&[
        ("root", "use a\nuse b\nrel derived result = select { id: x.id } from x in rows\nrel derived arows = select { id: x.id } from x in a::rows\nrel derived brows = select { id: x.id } from x in b::rows"),
        ("a", "export rel input rows: { id: Str } key id"),
        ("b", "export rel input rows: { id: Str } key id"),
    ]).link().unwrap();
    assert!(lower_relations(&program)
        .unwrap_err()
        .to_string()
        .contains("root::rows"));
}

#[test]
fn self_join_preserves_bindings_and_same_side_equality_remains_a_filter() {
    let dag = dag("rel input rows: { id: Str, parent: Str } key id\nrel derived result = select { id: child.id, parent: parent.id } from child in rows, parent in rows where child.parent == parent.id and parent.id == parent.parent").unwrap();
    assert_eq!(
        dag.nodes
            .iter()
            .filter(|node| matches!(node, OperatorNode::Bind { .. }))
            .count(),
        2
    );
    let join = dag
        .nodes
        .iter()
        .find_map(|node| match node {
            OperatorNode::EquiJoin {
                left_keys,
                right_keys,
                ..
            } => Some((left_keys, right_keys)),
            _ => None,
        })
        .unwrap();
    assert_eq!(join.0.len(), 1);
    assert_eq!(join.0[0].binding, "child");
    assert_eq!(join.1[0].binding, "parent");
    assert!(dag
        .nodes
        .iter()
        .any(|node| matches!(node, OperatorNode::Filter { .. })));
}

#[test]
fn same_side_equality_cannot_license_cartesian_product() {
    let error = dag("rel input rows: { id: Str, parent: Str } key id\nrel derived result = select { id: child.id } from child in rows, parent in rows where parent.id == parent.parent").unwrap_err();
    assert!(
        error.contains("lacks an indexed equality predicate"),
        "{error}"
    );
}

#[test]
fn grouped_projection_contains_only_bound_keys_and_counts() {
    let dag = dag("rel input rows: { id: Str, category: Str } key id\nrel derived result = select { category: x.category, count: count() } from x in rows group by x.category").unwrap();
    assert!(dag
        .nodes
        .iter()
        .all(|node| !matches!(node, OperatorNode::Project { .. })));
    let projections = dag
        .nodes
        .iter()
        .find_map(|node| match node {
            OperatorNode::GroupedCount { projections, .. } => Some(projections),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        projections,
        &vec![
            ("category".into(), GroupProjection::Key(0)),
            ("count".into(), GroupProjection::Count)
        ]
    );
    for projection in ["{ id: x.id, count: count() }", "{ count: count() + 1 }"] {
        let error = self::dag(&format!("rel input rows: {{ id: Str, category: Str }} key id\nrel derived result = select {projection} from x in rows group by x.category")).unwrap_err();
        assert!(
            error.contains("must be an exact group key or count()"),
            "{error}"
        );
    }
}

#[test]
fn invalid_binding_field_and_duplicate_alias_fail_closed() {
    for query in [
        "select { id: missing.id } from x in rows",
        "select { id: x.missing } from x in rows",
        "select { id: x.id } from x in rows, x in rows where x.id == x.id",
        "select { id: x.id, id: x.id } from x in rows",
    ] {
        assert!(
            dag(&format!(
                "rel input rows: {{ id: Str }} key id\nrel derived bad = {query}"
            ))
            .is_err(),
            "{query}"
        );
    }
}

#[test]
fn root_let_and_decide_cannot_bypass_export_visibility() {
    for root in [
        "use lib\nlet x = lib::hidden(1)",
        "use lib\nrel input rows: { id: Int } key id\ndecide choice for row in rows per id { propose p priority 1 when true = lib::hidden(row.id) }",
    ] {
        // The first case specifically exercises the former root-item bypass.
        let graph = graph(&[("root", root), ("lib", "fn hidden(x: Int): Int = x")]);
        assert!(matches!(graph.link(), Err(ModuleLinkError::NonExportedAccess { .. })));
    }
}

#[test]
fn validate_unused_contracts_before_pruning_and_reject_duplicate_declarations() {
    assert!(graph(&[
        ("root", "use lib\nfn run(): Int = 1"),
        ("lib", "fn broken(x: Int, x: Int): Int = x")
    ])
    .link()
    .is_err());
    assert!(graph(&[("root", "fn same(): Int = 1\nfn same(): Int = 2")])
        .link()
        .is_err());
}

#[test]
fn exported_schema_changes_invalidate_interface() {
    let first = graph(&[("root", "export config Row = { id: Str }")]).manifest("brix.world@1");
    let next =
        graph(&[("root", "export config Row = { id: Str, total: Int }")]).manifest("brix.world@1");
    assert_ne!(
        first.modules["root"].interface_digest,
        next.modules["root"].interface_digest
    );
}

#[test]
fn loader_counts_in_flight_modules_and_checks_actual_bytes() {
    let sources = BTreeMap::from([
        ("root", "use a"),
        ("a", "use b"),
        ("b", "fn value(): Int = 1"),
    ]);
    let error = ModuleGraph::load(
        "root",
        &|name: &str| sources.get(name).map(|s| s.to_string()),
        ModuleLoaderLimits {
            max_import_modules: 2,
            ..ModuleLoaderLimits::default()
        },
    )
    .unwrap_err();
    assert!(matches!(error, ModuleLinkError::ModuleCountExceeded { .. }));
    let loader = SizedLoader::new(
        |_: &str| Some(1),
        |_: &str| Some("fn value(): Int = 123".to_string()),
    );
    assert!(matches!(
        ModuleGraph::load(
            "root",
            &loader,
            ModuleLoaderLimits {
                max_module_source_bytes: 8,
                ..ModuleLoaderLimits::default()
            }
        ),
        Err(ModuleLinkError::ModuleBytesExceeded { .. })
    ));
}
