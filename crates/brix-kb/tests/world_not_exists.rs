use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use brix_canon::{Digest, Domain};
use brix_kb::world::audit::WorldAuditDecodeLimits;
use brix_kb::world::audit::{build_genesis_bundle_from_session, ExecProfileV1, ProgramClosureV1};
use brix_kb::world::verify::{verify_world_audit_bundle, VerifyOptions};
use brix_kb::world::{
    RelationDecl, TupleRecord, Value, WorldBatch, WorldBatchOp, WorldKey, WorldManifest,
    WorldNetwork, WorldSession, WorldTuple,
};
use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};
use brix_lower::relation_dag::{OperatorId, OperatorNode};

const SOURCE: &str = r#"
rel input orders: { id: Str } key id
rel input quote: { id: Str, order_id: Str } key id
rel derived order_quote_counts =
    select { id: o.id, count: count_or_zero() }
    from o in orders, q in quote where q.order_id == o.id group by o.id
decide availability for c in order_quote_counts per id {
    propose missing_calculation priority 1 when c.count == 0 = "MissingCalculation"
    propose quoted priority 2 when c.count > 0 = "Quoted"
}
"#;
const ANTI_SOURCE: &str = r#"
rel input orders: { id: Str } key id
rel input quote: { id: Str, order_id: Str } key id
rel derived order_quote_counts =
    select { id: o.id, count: count_or_zero() }
    from o in orders, q in quote where q.order_id == o.id group by o.id
"#;

fn network(source: &str) -> WorldNetwork {
    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), source.to_string());
    let loader = |name: &str| sources.get(name).cloned();
    let linked = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .expect("loads")
        .link()
        .expect("links");
    WorldNetwork::from_program(&linked).expect("network")
}

fn row(fields: &[(&str, &str)]) -> WorldTuple {
    let mut record = TupleRecord::new();
    for (name, value) in fields {
        record.set_str(*name, value);
    }
    record.to_tuple()
}

fn upsert(relation: &str, key: &str, fields: &[(&str, &str)]) -> WorldBatchOp {
    WorldBatchOp::Upsert {
        relation: format!("root::{relation}"),
        key: WorldKey::from_str(key),
        tuple: row(fields),
    }
}

fn remove(relation: &str, key: &str) -> WorldBatchOp {
    WorldBatchOp::Remove {
        relation: format!("root::{relation}"),
        key: WorldKey::from_str(key),
    }
}

fn apply(
    network: &mut WorldNetwork,
    revision: u64,
    ops: Vec<WorldBatchOp>,
) -> brix_kb::world::NetworkDeltaReport {
    network
        .apply_batch(&WorldBatch::new(revision, format!("r{revision}"), ops))
        .expect("batch")
}

fn assert_reference(network: &WorldNetwork) {
    network
        .verify_differential_correctness()
        .expect("independent reference agrees");
}

fn counts(network: &WorldNetwork) -> Vec<(String, i64)> {
    let mut out = network
        .get_derived_tuples("root::order_quote_counts")
        .expect("count relation")
        .into_iter()
        .map(|tuple| {
            let count = Value::from_typed_bytes(
                tuple.get("count").unwrap(),
                &brix_syntax::ast::Ty::Named("Int".into()),
            )
            .unwrap()
            .as_int()
            .unwrap();
            (tuple.get_str("id").unwrap().to_string(), count)
        })
        .collect::<Vec<_>>();
    out.sort();
    out
}

struct TestDir(PathBuf);

impl TestDir {
    fn new(tag: &str) -> Self {
        Self(std::env::temp_dir().join(format!("brix_r4_not_exists_{tag}_{}", std::process::id())))
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn persistent_world(tag: &str, source: &str) -> (TestDir, WorldSession, BTreeMap<String, String>) {
    let dir = TestDir::new(tag);
    let manifest = WorldManifest::new(
        "r4-not-exists",
        "2026-10-09T00:00:00Z",
        Digest::of(Domain::Value, tag.as_bytes()),
        vec![
            RelationDecl::new("root::orders", vec!["id".into()], vec![], vec![]),
            RelationDecl::new(
                "root::quote",
                vec!["id".into()],
                vec!["order_id".into()],
                vec![],
            ),
        ],
    );
    let sources = BTreeMap::from([("root".to_string(), source.to_string())]);
    let mut session = WorldSession::create(&dir.0, manifest).expect("create world");
    session
        .save_program_closure("root", &sources)
        .expect("bind anti-join program");
    (dir, session, sources)
}

fn persisted_batch(session: &mut WorldSession, revision: u64, ops: Vec<WorldBatchOp>) {
    session
        .apply_batch(WorldBatch::new(revision, format!("r4-{revision}"), ops))
        .expect("persist batch");
}

#[test]
fn anti_join_tracks_zero_one_multiple_one_zero_and_parent_removal() {
    let mut net = network(SOURCE);
    apply(&mut net, 0, vec![upsert("orders", "o1", &[("id", "o1")])]);
    assert_eq!(counts(&net), [("o1".into(), 0)]);
    assert_eq!(
        net.get_settlement("root::availability", "o1")
            .unwrap()
            .candidate_name,
        "missing_calculation"
    );

    apply(
        &mut net,
        1,
        vec![upsert("quote", "q1", &[("id", "q1"), ("order_id", "o1")])],
    );
    assert_eq!(counts(&net), [("o1".into(), 1)]);
    assert_eq!(
        net.get_settlement("root::availability", "o1")
            .unwrap()
            .candidate_name,
        "quoted"
    );
    apply(
        &mut net,
        2,
        vec![upsert("quote", "q2", &[("id", "q2"), ("order_id", "o1")])],
    );
    assert_eq!(counts(&net), [("o1".into(), 2)]);
    apply(&mut net, 3, vec![remove("quote", "q1")]);
    assert!(
        counts(&net) == [("o1".into(), 1)],
        "one remaining quote preserves the parent count"
    );
    apply(&mut net, 4, vec![remove("quote", "q2")]);
    assert_eq!(counts(&net), [("o1".into(), 0)]);
    assert_eq!(
        net.get_settlement("root::availability", "o1")
            .unwrap()
            .candidate_name,
        "missing_calculation"
    );
    apply(&mut net, 5, vec![remove("orders", "o1")]);
    assert!(counts(&net).is_empty());
    apply(&mut net, 6, vec![upsert("orders", "o1", &[("id", "o1")])]);
    assert_eq!(counts(&net), [("o1".into(), 0)]);
}

#[test]
fn simultaneous_parent_and_right_insert_is_atomic_and_unrelated_rows_do_not_match() {
    let mut net = network(ANTI_SOURCE);
    let report = apply(
        &mut net,
        0,
        vec![
            upsert("orders", "o1", &[("id", "o1")]),
            upsert("quote", "q1", &[("id", "q1"), ("order_id", "o1")]),
            upsert("orders", "o2", &[("id", "o2")]),
            upsert("quote", "q2", &[("id", "q2"), ("order_id", "other")]),
        ],
    );
    assert_eq!(counts(&net), [("o1".into(), 1), ("o2".into(), 0)]);
    let anti_op = net
        .dag
        .nodes
        .iter()
        .position(|node| matches!(node, OperatorNode::ParentCount { .. }))
        .unwrap();
    let anti_work = report
        .diagnostics
        .operator_work
        .get(&OperatorId(anti_op))
        .copied()
        .unwrap_or_default();
    assert!((4..=6).contains(&anti_work));
    assert_reference(&net);
}

#[test]
fn unrelated_right_growth_uses_only_the_changed_key() {
    let mut net = network(ANTI_SOURCE);
    let parents = (0..100)
        .map(|i| {
            let id = format!("order-{i}");
            upsert("orders", &id, &[("id", &id)])
        })
        .collect();
    apply(&mut net, 0, parents);
    let report = apply(
        &mut net,
        1,
        vec![upsert(
            "quote",
            "other",
            &[("id", "other"), ("order_id", "absent")],
        )],
    );
    let anti_op = net
        .dag
        .nodes
        .iter()
        .position(|node| matches!(node, OperatorNode::ParentCount { .. }))
        .unwrap();
    assert_eq!(
        report.diagnostics.operator_work.get(&OperatorId(anti_op)),
        Some(&1)
    );
    assert_eq!(counts(&net).len(), 100);
}

#[test]
fn absence_count_replays_in_audit_bundle_and_survives_reopen() {
    let (dir, mut world, sources) = persistent_world("audit", ANTI_SOURCE);
    persisted_batch(&mut world, 0, vec![upsert("orders", "o1", &[("id", "o1")])]);
    world
        .verify_oracle()
        .expect("zero-count parent is retained");
    persisted_batch(
        &mut world,
        1,
        vec![upsert("quote", "q1", &[("id", "q1"), ("order_id", "o1")])],
    );
    world.verify_oracle().expect("positive count agrees");
    persisted_batch(
        &mut world,
        2,
        vec![upsert("quote", "q2", &[("id", "q2"), ("order_id", "o1")])],
    );
    persisted_batch(&mut world, 3, vec![remove("quote", "q1")]);
    persisted_batch(&mut world, 4, vec![remove("quote", "q2")]);
    world
        .verify_oracle()
        .expect("last removal restores zero count");

    let closure = ProgramClosureV1 {
        root_module: "root".into(),
        sources: sources.into_iter().collect(),
        program_manifest_digest: world.manifest().program_digest,
    };
    drop(world);

    let reopened = WorldSession::open(&dir.0).expect("reopen world");
    let bundle = build_genesis_bundle_from_session(&reopened, closure, ExecProfileV1::default())
        .expect("export genesis audit bundle");
    let opts = VerifyOptions {
        expect_head: reopened.current_revision_digest().unwrap(),
        expect_program: reopened.manifest().program_digest,
        trust_checkpoint: None,
        limits: WorldAuditDecodeLimits::default(),
        max_work: None,
    };
    let report = verify_world_audit_bundle(&bundle, &opts).expect("independent audit replay");
    assert_eq!(report.head_digest, opts.expect_head);
    assert_eq!(report.work.revisions_replayed, 5);
    reopened.verify_oracle().expect("reopened network agrees");

    let old = reopened
        .pin_revision(1)
        .expect("historical zero-count revision");
    assert!(old
        .get("root::orders", &WorldKey::from_str("o1"))
        .unwrap()
        .is_some());
    let quoted = reopened
        .pin_revision(2)
        .expect("historical positive-count revision");
    assert!(quoted
        .get("root::quote", &WorldKey::from_str("q1"))
        .unwrap()
        .is_some());
}

#[test]
fn decided_absence_fallback_flips_back_after_last_child_and_reopens_historically() {
    let (dir, mut world, _) = persistent_world("decision", SOURCE);
    persisted_batch(&mut world, 0, vec![upsert("orders", "o1", &[("id", "o1")])]);
    assert!(world
        .historical_settlement(1, Some("root::availability"), "o1")
        .unwrap()
        .is_some());
    persisted_batch(
        &mut world,
        1,
        vec![upsert("quote", "q1", &[("id", "q1"), ("order_id", "o1")])],
    );
    assert_eq!(
        world
            .historical_settlement(2, Some("root::availability"), "o1")
            .unwrap()
            .unwrap()
            .1
            .candidate_name,
        "quoted"
    );
    persisted_batch(&mut world, 2, vec![remove("quote", "q1")]);
    assert!(world
        .historical_settlement(3, Some("root::availability"), "o1")
        .unwrap()
        .is_some());
    drop(world);

    let reopened = WorldSession::open(&dir.0).expect("reopen decision world");
    assert!(reopened
        .historical_explain_decision_for(1, "root::availability", "o1")
        .unwrap()
        .is_some());
    assert!(reopened
        .historical_explain_decision_for(2, "root::availability", "o1")
        .unwrap()
        .is_some_and(|e| e.candidates.iter().any(|c| c.name == "quoted")));
    assert!(reopened
        .historical_explain_decision_for(3, "root::availability", "o1")
        .unwrap()
        .is_some());
}
