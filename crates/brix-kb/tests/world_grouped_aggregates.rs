use brix_kb::world::audit::{
    build_checkpoint_bundle_from_session, ExecProfileV1, ProgramClosureV1,
};
use brix_kb::world::verify::{verify_world_audit_bundle, VerifyOptions};
use brix_kb::world::{reference, TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldSession};
use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};
use std::collections::BTreeMap;
use std::fs;

const SOURCE: &str = r#"
rel input items: { id: Str, parent: Str, amount: Int, money: Decimal, rank: Int, label: Str } key id
rel derived totals = select {
    parent: x.parent,
    total: sum(x.amount),
    money: sum(x.money),
    low: min(x.rank),
    high: max(x.rank),
    first: min(x.label),
    last: max(x.label),
    count: count()
} from x in items group by x.parent
decide item_decision for i in items per id {
    propose present priority 1 when true = i.amount
}
"#;

fn sources() -> BTreeMap<String, String> {
    BTreeMap::from([("root".to_string(), SOURCE.to_string())])
}

fn item(id: &str, parent: &str, amount: i64, money: &str, rank: i64, label: &str) -> WorldBatchOp {
    let mut row = TupleRecord::new();
    row.set_str("id", id);
    row.set_str("parent", parent);
    row.set_str("amount", &amount.to_string());
    row.set_str("money", money);
    row.set_str("rank", &rank.to_string());
    row.set_str("label", label);
    WorldBatchOp::Upsert {
        relation: "root::items".into(),
        key: WorldKey::from_str(id),
        tuple: row.to_tuple(),
    }
}

fn linked_network() -> (
    brix_lower::module_graph::LinkedProgram,
    brix_kb::world::WorldNetwork,
) {
    let sources = sources();
    let loader = |name: &str| sources.get(name).cloned();
    let linked = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .unwrap()
        .link()
        .unwrap();
    let network = brix_kb::world::WorldNetwork::from_program(&linked).unwrap();
    (linked, network)
}

fn grouped(network: &brix_kb::world::WorldNetwork) -> BTreeMap<String, TupleRecord> {
    network.current_state().unwrap().derived_relations["root::totals"]
        .iter()
        .map(|tuple| {
            let tuple = tuple.to_tuple();
            let row = TupleRecord::from_tuple(&tuple).unwrap();
            (row.get_str("parent").unwrap().to_owned(), row)
        })
        .collect()
}

#[test]
fn exact_aggregates_update_incrementally_and_match_independent_reference() {
    let (_linked, mut net) = linked_network();
    let initial = vec![
        item("i1", "p1", 7, "1.20", 5, "z"),
        item("i2", "p1", -2, "2.30", 2, "a"),
        item("i3", "p2", 4, "-1.5", 9, "m"),
    ];
    net.apply_ops(&initial).unwrap();
    let rows = grouped(&net);
    let p1 = &rows["p1"];
    assert_eq!(p1.get_str("total"), Some("5"));
    assert_eq!(p1.get_str("money"), Some("3.5"));
    assert_eq!(p1.get_str("low"), Some("2"));
    assert_eq!(p1.get_str("high"), Some("5"));
    assert_eq!(p1.get_str("first"), Some("a"));
    assert_eq!(p1.get_str("last"), Some("z"));
    assert_eq!(p1.get_str("count"), Some("2"));

    let sources = sources();
    let loader = |name: &str| sources.get(name).cloned();
    let linked = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .unwrap()
        .link()
        .unwrap();
    let oracle = reference::from_program(&linked).unwrap();
    let current = net.current_state().unwrap();
    let expected = reference::evaluate(&oracle, &current.base_relations).unwrap();
    assert_eq!(current.derived_relations, expected.derived_relations);

    net.apply_ops(&[WorldBatchOp::Remove {
        relation: "root::items".into(),
        key: WorldKey::from_str("i2"),
    }])
    .unwrap();
    let p1 = &grouped(&net)["p1"];
    assert_eq!(p1.get_str("total"), Some("7"));
    assert_eq!(p1.get_str("money"), Some("1.2"));
    assert_eq!(p1.get_str("low"), Some("5"));
    assert_eq!(p1.get_str("high"), Some("5"));
    assert_eq!(p1.get_str("first"), Some("z"));
    assert_eq!(p1.get_str("last"), Some("z"));
    net.verify_differential_correctness().unwrap();
}

#[test]
fn aggregate_results_are_independent_of_input_insertion_order() {
    let (_, mut first) = linked_network();
    let (_, mut second) = linked_network();
    let rows = [
        item("i1", "p", i64::MAX, "0.1", 1, "a"),
        item("i2", "p", -1, "0.2", 2, "b"),
        item("i3", "p", 1, "0.3", 3, "c"),
    ];
    first.apply_ops(&rows).unwrap();
    second
        .apply_ops(&rows.into_iter().rev().collect::<Vec<_>>())
        .unwrap();
    assert_eq!(grouped(&first), grouped(&second));
}

#[test]
fn integer_sum_overflow_is_rejected_regardless_of_insertion_order() {
    for rows in [
        vec![
            item("i1", "p", i64::MAX, "0", 1, "a"),
            item("i2", "p", 1, "0", 2, "b"),
        ],
        vec![
            item("i2", "p", 1, "0", 2, "b"),
            item("i1", "p", i64::MAX, "0", 1, "a"),
        ],
    ] {
        let (_, mut net) = linked_network();
        let err = net.apply_ops(&rows).unwrap_err();
        assert!(err.to_string().contains("grouped Int sum overflow"));
    }
}

#[test]
fn atomic_batches_narrow_only_the_final_int_and_decimal_totals() {
    let max = i64::MAX;
    let decimal_max = "170141183460469231731687303715884105727";
    let first_order = vec![
        item("i1", "p", max, decimal_max, 1, "a"),
        item("i2", "p", 1, "0.1", 2, "b"),
        item("i3", "p", -1, "-0.10", 3, "c"),
    ];
    let second_order = vec![
        first_order[1].clone(),
        first_order[2].clone(),
        first_order[0].clone(),
    ];
    let (_, mut first) = linked_network();
    let (_, mut second) = linked_network();
    first.apply_ops(&first_order).unwrap();
    second.apply_ops(&second_order).unwrap();
    let expected = grouped(&first)["p"].clone();
    assert_eq!(expected.get_str("total"), Some("9223372036854775807"));
    assert_eq!(expected.get_str("money"), Some(decimal_max));
    assert_eq!(grouped(&second), grouped(&first));

    let (_, mut update_then_remove) = linked_network();
    let (_, mut remove_then_update) = linked_network();
    let baseline = vec![first_order[0].clone()];
    update_then_remove.apply_ops(&baseline).unwrap();
    remove_then_update.apply_ops(&baseline).unwrap();
    let update_batch = vec![first_order[1].clone(), first_order[2].clone()];
    update_then_remove.apply_ops(&update_batch).unwrap();
    remove_then_update
        .apply_ops(&update_batch.into_iter().rev().collect::<Vec<_>>())
        .unwrap();
    assert_eq!(grouped(&update_then_remove), grouped(&remove_then_update));

    let adjustment = vec![
        item("i2", "p", 2, "0.2", 2, "b"),
        WorldBatchOp::Remove {
            relation: "root::items".into(),
            key: WorldKey::from_str("i3"),
        },
        item("i4", "p", -2, "-0.2", 4, "d"),
    ];
    update_then_remove.apply_ops(&adjustment).unwrap();
    remove_then_update
        .apply_ops(&adjustment.into_iter().rev().collect::<Vec<_>>())
        .unwrap();
    assert_eq!(grouped(&update_then_remove), grouped(&remove_then_update));
    assert_eq!(
        grouped(&update_then_remove)["p"].get_str("total"),
        Some("9223372036854775807")
    );
    assert_eq!(
        grouped(&update_then_remove)["p"].get_str("money"),
        Some(decimal_max)
    );
}

#[test]
fn sum_f64_is_refused_during_lowering() {
    let src = "rel input vals: { id: Str, x: F64 } key id\nrel derived totals = select { total: sum(v.x) } from v in vals\n";
    let map = BTreeMap::from([("root".to_string(), src.to_string())]);
    let loader = |name: &str| map.get(name).cloned();
    let linked = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .unwrap()
        .link()
        .unwrap();
    let err = brix_lower::relation_dag::lower_relations(&linked).unwrap_err();
    assert!(err.to_string().contains("sum(F64) is refused"));
}

#[test]
fn aggregate_journal_passes_independent_audit_verification() {
    let root =
        std::env::temp_dir().join(format!("brix_group_aggregate_audit_{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let sources = sources();
    let mut world = WorldSession::from_program_with_sources(&root, "root", &sources).unwrap();
    world
        .apply_batch(WorldBatch::new(
            0,
            "aggregate-r1",
            vec![
                item("i1", "p", 7, "1.20", 5, "z"),
                item("i2", "p", -2, "2.30", 2, "a"),
            ],
        ))
        .unwrap();
    world
        .apply_batch(WorldBatch::new(
            1,
            "aggregate-r2",
            vec![WorldBatchOp::Remove {
                relation: "root::items".into(),
                key: WorldKey::from_str("i2"),
            }],
        ))
        .unwrap();
    let program = ProgramClosureV1 {
        root_module: "root".into(),
        sources: sources.into_iter().collect(),
        program_manifest_digest: world.manifest().program_digest,
    };
    let bundle =
        build_checkpoint_bundle_from_session(&world, 1, program, ExecProfileV1::default()).unwrap();
    let options = VerifyOptions {
        expect_head: world.current_revision_digest().unwrap(),
        expect_program: world.manifest().program_digest,
        trust_checkpoint: match &bundle.scope {
            brix_kb::world::audit::ScopeV1::Checkpoint {
                revision_digest, ..
            } => Some(*revision_digest),
            _ => None,
        },
        limits: Default::default(),
        max_work: None,
    };
    verify_world_audit_bundle(&bundle, &options).unwrap();
    drop(world);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn ordered_extrema_survive_large_group_restart_and_retraction() {
    let root = std::env::temp_dir().join(format!(
        "brix_group_extrema_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let sources = sources();
    let mut world = WorldSession::from_program_with_sources(&root, "root", &sources).unwrap();
    let rows = (0..384)
        .map(|value| {
            item(
                &format!("i{value:03}"),
                "p",
                1,
                "0.1",
                value,
                &format!("v{value:03}"),
            )
        })
        .collect();
    world
        .apply_batch(WorldBatch::new(0, "extrema-fill", rows))
        .unwrap();
    drop(world);

    let mut world = WorldSession::open(&root).unwrap();
    let rows = world.get_derived("totals").unwrap().unwrap();
    let row = TupleRecord::from_tuple(&rows[0].to_tuple()).unwrap();
    assert_eq!(row.get_str("low"), Some("0"));
    assert_eq!(row.get_str("high"), Some("383"));
    assert_eq!(row.get_str("first"), Some("v000"));
    assert_eq!(row.get_str("last"), Some("v383"));

    world
        .apply_batch(WorldBatch::new(
            1,
            "extrema-retract",
            vec![
                WorldBatchOp::Remove {
                    relation: "root::items".into(),
                    key: WorldKey::from_str("i000"),
                },
                WorldBatchOp::Remove {
                    relation: "root::items".into(),
                    key: WorldKey::from_str("i383"),
                },
            ],
        ))
        .unwrap();
    let rows = world.get_derived("totals").unwrap().unwrap();
    let row = TupleRecord::from_tuple(&rows[0].to_tuple()).unwrap();
    assert_eq!(row.get_str("low"), Some("1"));
    assert_eq!(row.get_str("high"), Some("382"));
    assert_eq!(row.get_str("first"), Some("v001"));
    assert_eq!(row.get_str("last"), Some("v382"));
    drop(world);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn aggregate_update_work_does_not_grow_with_group_cardinality() {
    fn measured_update(size: usize) -> u64 {
        let (_, mut network) = linked_network();
        let mut rows = Vec::with_capacity(size);
        for value in 0..size {
            rows.push(item(
                &format!("i{value:04}"),
                "p",
                1,
                "0.1",
                value as i64,
                &format!("v{value:04}"),
            ));
        }
        network.apply_ops(&rows).unwrap();
        let target = size / 2;
        let report = network
            .apply_ops(&[item(
                &format!("i{target:04}"),
                "p",
                2,
                "0.2",
                target as i64,
                &format!("v{target:04}"),
            )])
            .unwrap();
        report.diagnostics.operator_work.values().sum()
    }

    let large_group_work = measured_update(384);
    assert!(
        large_group_work < 384 / 2,
        "treap work should stay below a linear scan of the ordered values; got {large_group_work}"
    );
}
