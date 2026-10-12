use brix_canon::{Digest, Domain};
use brix_kb::world::audit::{
    build_checkpoint_bundle_from_session, ExecProfileV1, ProgramClosureV1,
};
use brix_kb::world::verify::{verify_world_audit_bundle, VerifyOptions};
use brix_kb::world::{
    RelationDecl, TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldManifest, WorldNetwork,
    WorldSession,
};
use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};
use std::collections::BTreeMap;
use std::fs;

const SOURCE: &str = r#"
rel input orders: { id: Str, quote: Int } key id
rel input gates: { id: Str, active: Int } key id
decide approve for o in orders per id {
    propose accepted priority 1 when o.quote > 0 = o.quote
}
rel derived approved_orders = select { id: a.id, quote: a.value } from a in approve
rel derived review_inputs = select { id: a.id, quote: a.quote, active: g.active }
    from a in approved_orders, g in gates where a.id == g.id
decide review for a in review_inputs per id {
    propose reviewed priority 2 when a.quote > 0 = a.quote + a.active
}
rel derived reviewed_orders = select { id: r.id, quote: r.value } from r in review
decide dispatch for r in reviewed_orders per id {
    propose dispatch priority 3 when r.quote > 0 = r.quote
}
"#;

fn network() -> WorldNetwork {
    let sources = BTreeMap::from([("root".to_string(), SOURCE.to_string())]);
    let loader = |name: &str| sources.get(name).cloned();
    let linked = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .unwrap()
        .link()
        .unwrap();
    WorldNetwork::from_program(&linked).unwrap()
}

fn order(quote: i64) -> WorldBatchOp {
    order_for("o-1", quote)
}

fn order_for(id: &str, quote: i64) -> WorldBatchOp {
    let mut row = TupleRecord::new();
    row.set_str("id", id);
    row.set_str("quote", &quote.to_string());
    WorldBatchOp::Upsert {
        relation: "root::orders".into(),
        key: WorldKey::from_str(id),
        tuple: row.to_tuple(),
    }
}

fn gate_for(id: &str, active: i64) -> WorldBatchOp {
    let mut row = TupleRecord::new();
    row.set_str("id", id);
    row.set_str("active", &active.to_string());
    WorldBatchOp::Upsert {
        relation: "root::gates".into(),
        key: WorldKey::from_str(id),
        tuple: row.to_tuple(),
    }
}

#[test]
fn changing_one_entity_does_constant_operator_work_with_more_unrelated_rows() {
    fn measured_update(unrelated: usize) -> u64 {
        let mut network = network();
        let mut initial = Vec::with_capacity(unrelated + 1);
        initial.push(order_for("o-1", 10));
        initial.push(gate_for("o-1", 1));
        for idx in 0..unrelated {
            initial.push(order_for(&format!("unrelated-{idx}"), 10));
        }
        network.apply_ops(&initial).unwrap();
        let report = network.apply_ops(&[order_for("o-1", 11)]).unwrap();
        report.diagnostics.operator_work.values().sum()
    }

    let small = measured_update(0);
    let large = measured_update(1_000);
    assert_eq!(
        large, small,
        "unrelated resident rows must not add operator work"
    );
}

#[test]
fn winner_rows_feed_later_decisions_in_the_same_revision_and_retract_atomically() {
    let mut network = network();
    let first = network.apply_ops(&[order(17), gate_for("o-1", 1)]).unwrap();
    assert_eq!(first.revision, 1);
    assert!(first.settlements["root::approve"].contains_key("o-1"));
    assert!(first.settlements["root::review"].contains_key("o-1"));
    assert!(first.settlements["root::dispatch"].contains_key("o-1"));
    assert_eq!(
        network.current_state().derived_relations["root::approve"].len(),
        1
    );
    assert_eq!(
        network.current_state().derived_relations["root::approved_orders"].len(),
        1
    );
    assert_eq!(
        network.current_state().derived_relations["root::review"].len(),
        1
    );
    assert_eq!(
        network.current_state().derived_relations["root::reviewed_orders"].len(),
        1
    );
    assert_independent_reference_matches(&network);

    let old = network.clone();
    // Both the upstream decision's source and a second source of its downstream
    // join change in this same atomic revision.
    let changed = network.apply_ops(&[order(23), gate_for("o-1", 2)]).unwrap();
    assert_eq!(changed.revision, 2);
    assert!(changed.settlements["root::approve"].contains_key("o-1"));
    assert!(changed.settlements["root::review"].contains_key("o-1"));
    assert!(changed.settlements["root::dispatch"].contains_key("o-1"));
    assert_eq!(old.current_revision, 1);
    assert_eq!(
        old.current_state().derived_relations["root::approved_orders"].len(),
        1
    );
    let explanation = old
        .explain_decision_for("root::dispatch", "o-1")
        .expect("historical downstream explanation");
    assert!(explanation
        .contributing_facts
        .contains(&("root::orders".into(), WorldKey::from_str("o-1"))));
    assert_eq!(explanation.upstream_settlements.len(), 2);
    assert!(explanation.upstream_settlements.iter().any(|link| {
        link.decide == "root::approve" && link.entity_id == "o-1" && link.revision == 1
    }));
    assert!(explanation.upstream_settlements.iter().any(|link| {
        link.decide == "root::review" && link.entity_id == "o-1" && link.revision == 1
    }));
    assert_independent_reference_matches(&network);

    let expired = network.apply_ops(&[order(0)]).unwrap();
    assert_eq!(expired.revision, 3);
    assert!(expired.removed_settlements["root::approve"].contains("o-1"));
    assert!(expired.removed_settlements["root::review"].contains("o-1"));
    assert!(expired.removed_settlements["root::dispatch"].contains("o-1"));
    assert!(network.current_state().derived_relations["root::approve"].is_empty());
    assert!(network.current_state().derived_relations["root::approved_orders"].is_empty());
    assert!(network.current_state().derived_relations["root::review"].is_empty());
    assert!(network.current_state().derived_relations["root::reviewed_orders"].is_empty());
    assert_independent_reference_matches(&network);
}

fn assert_independent_reference_matches(network: &WorldNetwork) {
    let sources = BTreeMap::from([("root".to_string(), SOURCE.to_string())]);
    let loader = |name: &str| sources.get(name).cloned();
    let linked = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .unwrap()
        .link()
        .unwrap();
    let reference = brix_kb::world::reference::from_program(&linked).unwrap();
    let current = network.current_state();
    let expected =
        brix_kb::world::reference::evaluate(&reference, &current.base_relations).unwrap();
    assert_eq!(current.settlements.len(), expected.settlements.len());
    for (decide, entities) in &current.settlements {
        assert_eq!(entities.len(), expected.settlements[decide].len());
        for (entity, actual) in entities {
            let expected = &expected.settlements[decide][entity];
            assert_eq!(actual.candidate_name, expected.candidate_name);
            assert_eq!(actual.priority, expected.priority);
            assert_eq!(actual.phase, expected.phase);
            assert_eq!(actual.value.to_string(), expected.value.to_string());
            assert_eq!(actual.calendar_key, expected.calendar_key);
        }
    }
    assert_eq!(current.derived_relations, expected.derived_relations);
}

#[test]
fn decision_chain_survives_persistence_restart_and_verification() {
    let root = std::env::temp_dir().join(format!(
        "brix_decision_chain_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let manifest = WorldManifest::new(
        "decision_chain",
        "2026-10-08T00:00:00Z",
        Digest::of(Domain::Value, b"decision-chain-genesis"),
        vec![
            RelationDecl::new(
                "root::orders",
                vec!["id".into()],
                vec!["quote".into()],
                vec![],
            ),
            RelationDecl::new(
                "root::gates",
                vec!["id".into()],
                vec!["active".into()],
                vec![],
            ),
        ],
    );
    let mut world = WorldSession::create(&root, manifest).unwrap();
    world.set_network(network()).unwrap();
    let sources = BTreeMap::from([("root".into(), SOURCE.into())]);
    world.save_program_closure("root", &sources).unwrap();
    world
        .apply_batch(WorldBatch::new(
            0,
            "chain-r1",
            vec![order(17), gate_for("o-1", 1)],
        ))
        .unwrap();
    world
        .apply_batch(WorldBatch::new(
            1,
            "chain-r2",
            vec![order(23), gate_for("o-1", 2)],
        ))
        .unwrap();
    let current_explanation = world.explain_decision_for("root::dispatch", "o-1").unwrap();
    assert!(current_explanation
        .upstream_settlements
        .iter()
        .all(|link| link.revision == 2));
    assert_eq!(
        world
            .historical_settlement(1, Some("root::dispatch"), "o-1")
            .unwrap()
            .unwrap()
            .1
            .value
            .to_json(),
        serde_json::json!("18")
    );
    let program = ProgramClosureV1 {
        root_module: "root".into(),
        sources: sources.into_iter().collect(),
        program_manifest_digest: world.manifest().program_digest,
    };
    let bundle =
        build_checkpoint_bundle_from_session(&world, 1, program, ExecProfileV1::default()).unwrap();
    let verify = VerifyOptions {
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
    verify_world_audit_bundle(&bundle, &verify).unwrap();
    drop(world);

    let reopened = WorldSession::open(&root).unwrap();
    assert_eq!(
        reopened
            .historical_settlement(2, Some("root::dispatch"), "o-1")
            .unwrap()
            .unwrap()
            .1
            .value
            .to_json(),
        serde_json::json!("25")
    );
    let historical = reopened
        .historical_explain_decision_for(1, "root::dispatch", "o-1")
        .unwrap()
        .unwrap();
    assert_eq!(historical.upstream_settlements.len(), 2);
    assert!(historical
        .upstream_settlements
        .iter()
        .all(|link| link.revision == 1));
    assert!(historical
        .upstream_settlements
        .iter()
        .any(|link| { link.decide == "root::approve" && link.entity_id == "o-1" }));
    assert!(historical
        .upstream_settlements
        .iter()
        .any(|link| { link.decide == "root::review" && link.entity_id == "o-1" }));
    let explanation = reopened
        .explain_decision_for("root::dispatch", "o-1")
        .unwrap();
    assert_eq!(explanation.upstream_settlements.len(), 2);
    assert!(explanation
        .upstream_settlements
        .iter()
        .all(|link| link.revision == 2));
    let _ = fs::remove_dir_all(root);
}
