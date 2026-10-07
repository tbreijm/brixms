use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use brix_canon::{Digest, Domain};
use brix_kb::world::audit::{
    build_checkpoint_bundle_from_session, build_genesis_bundle_from_session,
    decode_world_audit_bundle_v1, ExecProfileV1, ProgramClosureV1, ScopeV1, WorldAuditDecodeLimits,
};
use brix_kb::world::verify::{verify_world_audit_bundle, VerifyError, VerifyOptions};
use brix_kb::world::{
    RelationDecl, TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldManifest, WorldSession,
};

const SOURCE: &str = r#"
rel input rows: { id: Str, owner: Str, amount: Int } key id
rel derived eligible = select { owner: r.owner, amount: r.amount } from r in rows where r.amount > 0
decide alpha for r in eligible per owner { propose allow priority 1 when true = r.amount }
decide omega for r in eligible per owner { propose label priority 2 when true = r.owner }
"#;

struct TestDir(PathBuf);
impl TestDir {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("brix_audit_tamper_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        Self(path)
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn upsert(id: &str, owner: &str, amount: i64) -> WorldBatchOp {
    let mut record = TupleRecord::new();
    record.set_str("id", id);
    record.set_str("owner", owner);
    record.set_str("amount", &amount.to_string());
    WorldBatchOp::Upsert {
        relation: "root::rows".into(),
        key: WorldKey::from_str(id),
        tuple: record.to_tuple(),
    }
}

fn batch(seq: u64, ops: Vec<WorldBatchOp>) -> WorldBatch {
    WorldBatch::new(seq, format!("batch-{seq}"), ops)
}

fn setup_world(name: &str) -> (TestDir, WorldSession, ProgramClosureV1, ExecProfileV1) {
    let dir = TestDir::new(name);
    let manifest = WorldManifest::new(
        "p6-tamper",
        "2026-10-05T00:00:00Z",
        Digest::of(Domain::Value, b"initial"),
        vec![RelationDecl::new(
            "root::rows",
            vec!["id".into()],
            vec!["owner".into(), "amount".into()],
            vec!["owner".into()],
        )],
    );
    let mut world = WorldSession::create(&dir.0, manifest).unwrap();
    let sources = BTreeMap::from([("root".into(), SOURCE.into())]);
    world.save_program_closure("root", &sources).unwrap();

    let program_manifest_digest = world.manifest().program_digest;
    let ep = ExecProfileV1::default();
    let pc = ProgramClosureV1 {
        root_module: "root".to_string(),
        sources: vec![("root".to_string(), SOURCE.to_string())],
        program_manifest_digest,
    };
    (dir, world, pc, ep)
}

fn make_options(head: Digest, program: Digest) -> VerifyOptions {
    VerifyOptions {
        expect_head: head,
        expect_program: program,
        trust_checkpoint: None,
        limits: WorldAuditDecodeLimits::default(),
        max_work: None,
    }
}

#[test]
fn review_checkpoint_omitted_relation_is_rejected() {
    let dir = TestDir::new("review_missing_rel");
    let source = "rel input rows: { id: Str, amount: Int } key id\ndecide choice for r in rows per id { propose allow priority 1 when false = r.amount }\n";
    let sources = BTreeMap::from([("root".into(), source.into())]);
    let mut world = WorldSession::from_program_with_sources(&dir.0, "root", &sources).unwrap();
    let mut rec = TupleRecord::new();
    rec.set_str("id", "a");
    rec.set_str("amount", "10");
    world
        .apply_batch(batch(
            0,
            vec![WorldBatchOp::Upsert {
                relation: "root::rows".into(),
                key: WorldKey::from_str("a"),
                tuple: rec.to_tuple(),
            }],
        ))
        .unwrap();
    let pc = ProgramClosureV1 {
        root_module: "root".into(),
        sources: sources.into_iter().collect(),
        program_manifest_digest: world.manifest().program_digest,
    };
    let mut bundle =
        build_checkpoint_bundle_from_session(&world, 1, pc.clone(), ExecProfileV1::default())
            .unwrap();
    let mut opts = make_options(bundle.head.revision_digest, pc.program_manifest_digest);
    opts.trust_checkpoint = Some(bundle.head.revision_digest);
    assert!(verify_world_audit_bundle(&bundle, &opts).is_ok());

    // Clear relations in checkpoint state: must be rejected because root::rows was omitted
    if let ScopeV1::Checkpoint { ref mut state, .. } = bundle.scope {
        state.relations.clear();
    }
    let decoded =
        decode_world_audit_bundle_v1(&bundle.encode(&opts.limits).unwrap(), &opts.limits).unwrap();
    let err = verify_world_audit_bundle(&decoded, &opts).unwrap_err();
    match err {
        VerifyError::RelationRootMismatch { relation, .. }
        | VerifyError::RelationCardinalityMismatch { relation, .. } => {
            assert_eq!(relation, "root::rows");
        }
        other => {
            panic!("expected RelationRootMismatch or RelationCardinalityMismatch, got {other:?}")
        }
    }
}

#[test]
fn review_checkpoint_foreign_execution_profile_is_rejected() {
    let (_dir, mut world, pc, ep) = setup_world("review_exec_profile");
    world
        .apply_batch(batch(0, vec![upsert("a", "alice", 10)]))
        .unwrap();
    let mut bundle = build_checkpoint_bundle_from_session(&world, 1, pc.clone(), ep).unwrap();
    let mut opts = make_options(bundle.head.revision_digest, pc.program_manifest_digest);
    opts.trust_checkpoint = Some(bundle.head.revision_digest);
    assert!(verify_world_audit_bundle(&bundle, &opts).is_ok());

    bundle.exec_profile.evaluator = "unsupported-evaluator@999".into();
    bundle.exec_profile.numeric_semantics = "different-arithmetic".into();
    bundle.exec_profile.module_loader_limits.depth = u64::MAX;
    let decoded =
        decode_world_audit_bundle_v1(&bundle.encode(&opts.limits).unwrap(), &opts.limits).unwrap();
    let err = verify_world_audit_bundle(&decoded, &opts).unwrap_err();
    match err {
        VerifyError::UnsupportedExecProfile(_) | VerifyError::ExecProfileMismatch { .. } => {}
        other => panic!("expected execution profile rejection, got {other:?}"),
    }
}

#[test]
fn review_mismatched_decision_delta_digest_is_rejected() {
    let (_dir, mut world, pc, ep) = setup_world("review_delta_binding");
    world
        .apply_batch(batch(0, vec![upsert("a", "alice", 10)]))
        .unwrap();
    let mut bundle = build_genesis_bundle_from_session(&world, pc.clone(), ep).unwrap();
    bundle.revisions[0].record.decision_delta_digest =
        Some(Digest::of(Domain::Value, b"wrong delta"));
    bundle.revisions[0].record.revision_digest =
        bundle.revisions[0].record.compute_digest_for_record();
    bundle.head.revision_digest = bundle.revisions[0].record.revision_digest;
    let opts = make_options(bundle.head.revision_digest, pc.program_manifest_digest);
    let decoded =
        decode_world_audit_bundle_v1(&bundle.encode(&opts.limits).unwrap(), &opts.limits).unwrap();
    let err = verify_world_audit_bundle(&decoded, &opts).unwrap_err();
    match err {
        VerifyError::DecisionDeltaMismatch { .. } => {}
        other => panic!("expected DecisionDeltaMismatch, got {other:?}"),
    }
}

#[test]
fn review_cumulative_work_bounds_reference_rows() {
    let (_dir, mut world, pc, ep) = setup_world("review_work_meter");
    for seq in 0..10 {
        world
            .apply_batch(batch(seq, vec![upsert("a", "alice", 10 + seq as i64)]))
            .unwrap();
    }
    let bundle = build_genesis_bundle_from_session(&world, pc.clone(), ep).unwrap();
    let mut opts = make_options(bundle.head.revision_digest, pc.program_manifest_digest);
    let report = verify_world_audit_bundle(&bundle, &opts).unwrap();

    let graph = brix_lower::module_graph::ModuleGraph::load(
        "root",
        &|_: &str| Some(SOURCE.to_owned()),
        Default::default(),
    )
    .unwrap();
    let prog = brix_kb::world::reference::from_program(&graph.link().unwrap()).unwrap();
    let mut rec = TupleRecord::new();
    rec.set_str("id", "a");
    rec.set_str("owner", "alice");
    rec.set_str("amount", "10");
    let relations = BTreeMap::from([(
        "root::rows".to_owned(),
        BTreeMap::from([(WorldKey::from_str("a"), rec.to_tuple())]),
    )]);
    let mut meter = brix_kb::world::reference::ReferenceWorkMeter::default();
    brix_kb::world::reference::evaluate_with_budget(&prog, &relations, &mut meter, None, 0)
        .unwrap();
    assert!(meter.rows_evaluated > 0);
    assert_eq!(
        report.work.rows_evaluated,
        10 * meter.rows_evaluated,
        "report must accumulate rows_evaluated across all 10 revisions"
    );

    // With rows_evaluated included in total_work(), setting max_work below actual total work must be rejected
    opts.max_work = Some(report.work.total_work() - 1);
    let err = verify_world_audit_bundle(&bundle, &opts).unwrap_err();
    assert!(
        matches!(err, VerifyError::BudgetExhausted),
        "expected BudgetExhausted, got {err:?}"
    );
}
