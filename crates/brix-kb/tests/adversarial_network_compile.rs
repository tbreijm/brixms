use brix_kb::world::network::WorldNetwork;
use brix_kb::world::{TupleRecord, WorldBatchOp, WorldKey};
use brix_lower::module_graph::{LinkedProgram, ModuleGraph, ModuleLoaderLimits};

fn legacy_apply(p: &LinkedProgram, with_flag: bool) {
    let mut n = WorldNetwork::new(brix_lower::lower_relations(p).unwrap());
    let mut t = TupleRecord::new();
    t.set_str("id", "x");
    if with_flag {
        t.set_str("flag", "true");
    }
    n.apply_ops(&[WorldBatchOp::Upsert {
        relation: "root::rows".into(),
        key: WorldKey::from_str("x"),
        tuple: t.to_tuple(),
    }])
    .unwrap();
    assert!(n.derived_relations.values().all(|r| r.len() == 1));
}

fn link(src: &str) -> LinkedProgram {
    let loader = |name: &str| {
        if name == "root" {
            Some(src.to_owned())
        } else {
            None
        }
    };
    ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .unwrap()
        .link()
        .unwrap()
}

#[test]
fn unqualified_projection_column() {
    let p = link(
        "rel input rows: { id: Str } key id\nrel derived out = select { id: id } from r in rows\n",
    );
    legacy_apply(&p, false);
    assert!(
        WorldNetwork::from_program(&p).is_ok(),
        "{:?}",
        WorldNetwork::from_program(&p)
    );
}

#[test]
fn match_only_projected_relation() {
    let p = link(
        "rel input rows: { id: Str, flag: Bool } key id\nrel derived a = select { value: match r.flag { true => 1 false => 0 } } from r in rows\nrel derived b = select { value: a.value } from a in a\n",
    );
    legacy_apply(&p, true);
    assert!(
        WorldNetwork::from_program(&p).is_ok(),
        "{:?}",
        WorldNetwork::from_program(&p)
    );
}
