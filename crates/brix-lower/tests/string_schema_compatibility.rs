use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};
use brix_lower::relation_dag::lower_relations;
use brix_syntax::ast::Ty;
use std::collections::BTreeMap;

fn linked(source: &str) -> brix_lower::module_graph::LinkedProgram {
    let sources = BTreeMap::from([("root".to_string(), source.to_string())]);
    ModuleGraph::load(
        "root",
        &|name: &str| sources.get(name).cloned(),
        ModuleLoaderLimits::default(),
    )
    .unwrap()
    .link()
    .unwrap()
}

#[test]
fn string_admission_does_not_strengthen_persisted_untyped_helper_schema() {
    let program = linked("rel input rows: { id: Str, value: Int } key id\nfn twice(x) = x + x\nrel derived result = select { value: twice(r.value) } from r in rows");
    let dag = lower_relations(&program).unwrap();
    assert_eq!(
        dag.relation_schemas["root::result"]["value"],
        Ty::Named("Any".into())
    );
}

#[test]
fn shared_numeric_helper_dag_is_checked_without_exponential_expansion() {
    let mut source =
        "rel input rows: { id: Str, value: Int } key id\nfn f0(x) = x + x\n".to_string();
    for index in 1..28 {
        source.push_str(&format!(
            "fn f{index}(x) = f{}(x) + f{}(x)\n",
            index - 1,
            index - 1
        ));
    }
    source.push_str("rel derived result = select { value: f27(r.value) } from r in rows");
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        sender
            .send(
                lower_relations(&linked(&source))
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
            )
            .unwrap()
    });
    receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("helper DAG must not expand exponentially")
        .unwrap();
}
