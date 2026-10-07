//! Tamper matrix test suite for persistent world audit verification (ADR-0046 P6 §3).
//!
//! Rejection at every layer: input tuple tampering, dependency tampering,
//! cache manipulation, secondary index forgery, checkpoint tampering,
//! delta reordering, historical decision tampering, foreign relations,
//! honest controls, historical retraction, and emptied history attacks.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use brix_canon::{Digest, Domain};
use brix_kb::world::audit::{
    build_checkpoint_bundle_from_session, build_genesis_bundle_from_session,
    decode_world_audit_bundle_v1, ExecProfileV1, ProgramClosureV1, ScopeV1, WorldAuditBundleError,
    WorldAuditDecodeLimits,
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
fn t01_input_tuple_edit_breaks_relation_root() {
    let (_dir, mut world, pc, ep) = setup_world("t01");
    world
        .apply_batch(batch(
            0,
            vec![upsert("a", "alice", 10), upsert("b", "bob", 20)],
        ))
        .unwrap();

    let mut bundle = build_genesis_bundle_from_session(&world, pc.clone(), ep).unwrap();
    let opts = make_options(bundle.head.revision_digest, pc.program_manifest_digest);

    // Honest verification passes
    assert!(verify_world_audit_bundle(&bundle, &opts).is_ok());

    // Tamper input tuple bytes in source delta
    let mut bad_rec = TupleRecord::new();
    bad_rec.set_str("id", "a");
    bad_rec.set_str("owner", "alice");
    bad_rec.set_str("amount", "999");
    bundle.revisions[0].source_delta[0].2 = Some(bad_rec.to_tuple());

    let err = verify_world_audit_bundle(&bundle, &opts).unwrap_err();
    assert!(
        matches!(err, VerifyError::RelationRootMismatch { ref relation, .. } if relation == "root::rows"),
        "expected RelationRootMismatch, got: {err:?}"
    );
}

#[test]
fn t02_dependency_source_edit_breaks_program_pin() {
    let (_dir, mut world, pc, ep) = setup_world("t02");
    world
        .apply_batch(batch(0, vec![upsert("a", "alice", 10)]))
        .unwrap();

    let mut bundle = build_genesis_bundle_from_session(&world, pc.clone(), ep).unwrap();
    let opts = make_options(bundle.head.revision_digest, pc.program_manifest_digest);

    // Tamper program source in the bundle
    bundle.program.sources[0].1 = SOURCE.replace("r.amount > 0", "r.amount > 50");

    let err = verify_world_audit_bundle(&bundle, &opts).unwrap_err();
    assert!(
        matches!(err, VerifyError::ProgramPinMismatch { .. }),
        "expected ProgramPinMismatch, got: {err:?}"
    );
}

#[test]
fn t03_cache_object_swap_is_corruption_not_evidence() {
    let (dir, mut world, pc, ep) = setup_world("t03");
    world
        .apply_batch(batch(0, vec![upsert("a", "alice", 10)]))
        .unwrap();

    let bundle = build_genesis_bundle_from_session(&world, pc.clone(), ep).unwrap();
    let opts = make_options(bundle.head.revision_digest, pc.program_manifest_digest);

    // Corrupt an object file in the world store
    let obj_dir = dir.0.join("objects");
    if obj_dir.exists() {
        for entry in fs::read_dir(&obj_dir).unwrap().flatten() {
            if entry.path().is_file() {
                fs::write(entry.path(), b"corrupted object payload").unwrap();
                break;
            }
        }
    }

    // World session reopen will fail or fail reading
    drop(world);
    let reopen = WorldSession::open(&dir.0);
    if let Ok(s) = reopen {
        let _ = s.get("root::rows", &WorldKey::from_str("a"));
    }

    // BUT audit bundle verification succeeds completely because it does not depend on store caches
    let report = verify_world_audit_bundle(&bundle, &opts).unwrap();
    assert_eq!(report.head_revision, 1);
}

#[test]
fn t04_index_root_forgery_fails_pin_then_rebuild() {
    let (_dir, mut world, pc, ep) = setup_world("t04");
    world
        .apply_batch(batch(
            0,
            vec![upsert("a", "alice", 10), upsert("b", "bob", 20)],
        ))
        .unwrap();

    let mut bundle = build_genesis_bundle_from_session(&world, pc.clone(), ep).unwrap();
    let opts = make_options(bundle.head.revision_digest, pc.program_manifest_digest);

    // Part A: Tamper secondary index root without updating revision digest
    let fake_digest = Digest::of(Domain::Value, b"forged-secondary-index-root");
    bundle.revisions[0]
        .record
        .secondary_index_roots
        .insert("root::rows:owner".to_string(), fake_digest);

    let err = verify_world_audit_bundle(&bundle, &opts).unwrap_err();
    assert!(
        matches!(err, VerifyError::RevisionDigestMismatch { .. }),
        "expected RevisionDigestMismatch, got: {err:?}"
    );

    // Part B: Re-seal the revision digest and pin to forged digest
    let forged_rev_digest = bundle.revisions[0].record.compute_digest_for_record();
    bundle.revisions[0].record.revision_digest = forged_rev_digest;
    bundle.head.revision_digest = forged_rev_digest;
    let forged_opts = make_options(forged_rev_digest, pc.program_manifest_digest);

    // Rebuilding the index from tuples catches the forgery
    let err2 = verify_world_audit_bundle(&bundle, &forged_opts).unwrap_err();
    assert!(
        matches!(err2, VerifyError::SecondaryIndexRootMismatch { ref index, .. } if index == "root::rows:owner"),
        "expected SecondaryIndexRootMismatch, got: {err2:?}"
    );
}

#[test]
fn t05_support_table_is_not_verifier_input() {
    let (_dir, mut world, pc, ep) = setup_world("t05");
    world
        .apply_batch(batch(0, vec![upsert("a", "alice", 10)]))
        .unwrap();

    let mut bundle = build_genesis_bundle_from_session(&world, pc.clone(), ep).unwrap();
    let opts = make_options(bundle.head.revision_digest, pc.program_manifest_digest);

    // Tamper the decision delta (claiming a fake candidate that support count would have allowed)
    if let Some((_, _, Some(ref mut dt))) = bundle.revisions[0].decision_delta.first_mut() {
        dt.candidate_name = "forged_candidate".to_string();
    }

    let err = verify_world_audit_bundle(&bundle, &opts).unwrap_err();
    assert!(
        matches!(err, VerifyError::DecisionDeltaMismatch { seq: 1 }),
        "expected DecisionDeltaMismatch, got: {err:?}"
    );
}

#[test]
fn t06_checkpoint_state_edit_rejected() {
    let (_dir, mut world, pc, ep) = setup_world("t06");
    world
        .apply_batch(batch(0, vec![upsert("a", "alice", 10)]))
        .unwrap();
    world
        .apply_batch(batch(1, vec![upsert("b", "bob", 20)]))
        .unwrap();

    let bundle = build_checkpoint_bundle_from_session(&world, 1, pc.clone(), ep).unwrap();
    let checkpoint_digest = match &bundle.scope {
        ScopeV1::Checkpoint {
            revision_digest, ..
        } => *revision_digest,
        _ => panic!("expected Checkpoint scope"),
    };

    // Case A: Untrusted checkpoint (no trust_checkpoint provided)
    let opts_untrusted = make_options(bundle.head.revision_digest, pc.program_manifest_digest);
    let err = verify_world_audit_bundle(&bundle, &opts_untrusted).unwrap_err();
    assert!(matches!(err, VerifyError::CheckpointUntrusted));

    // Case B: Trusted checkpoint passes
    let mut opts_trusted = opts_untrusted.clone();
    opts_trusted.trust_checkpoint = Some(checkpoint_digest);
    let report = verify_world_audit_bundle(&bundle, &opts_trusted).unwrap();
    assert_eq!(report.head_revision, 2);

    // Case C: Tamper checkpoint relation state
    let mut bad_bundle = bundle.clone();
    if let ScopeV1::Checkpoint { ref mut state, .. } = bad_bundle.scope {
        let mut bad_rec = TupleRecord::new();
        bad_rec.set_str("id", "a");
        bad_rec.set_str("owner", "alice");
        bad_rec.set_str("amount", "999");
        state.relations[0].1[0].1 = bad_rec.to_tuple();
    }
    let err = verify_world_audit_bundle(&bad_bundle, &opts_trusted).unwrap_err();
    assert!(
        matches!(err, VerifyError::RelationRootMismatch { seq: 1, .. }),
        "expected RelationRootMismatch on checkpoint tamper, got: {err:?}"
    );

    // Case D: Tamper checkpoint decision state
    let mut bad_bundle2 = bundle.clone();
    if let ScopeV1::Checkpoint { ref mut state, .. } = bad_bundle2.scope {
        state.decisions[0].2.candidate_name = "tampered_candidate".to_string();
    }
    let err2 = verify_world_audit_bundle(&bad_bundle2, &opts_trusted).unwrap_err();
    assert!(
        matches!(
            err2,
            VerifyError::DecisionRootMismatch { seq: 1, .. }
                | VerifyError::DecisionDeltaMismatch { seq: 1 }
        ),
        "expected Decision mismatch on checkpoint decision tamper, got: {err2:?}"
    );
}

#[test]
fn t07_trace_reorder_is_format_refusal() {
    let (_dir, mut world, pc, ep) = setup_world("t07");
    // Apply batch with multiple rows affecting decisions
    world
        .apply_batch(batch(
            0,
            vec![upsert("b", "bob", 20), upsert("a", "alice", 10)],
        ))
        .unwrap();

    let mut bundle = build_genesis_bundle_from_session(&world, pc.clone(), ep.clone()).unwrap();
    assert!(bundle.revisions[0].decision_delta.len() >= 2);

    // Swap decision delta entries out of canonical sorted order
    bundle.revisions[0].decision_delta.swap(0, 1);

    let limits = WorldAuditDecodeLimits::default();
    let bytes = bundle.encode(&limits).expect("raw encode");

    // Format refusal during decode before any semantic execution
    let decode_err = decode_world_audit_bundle_v1(&bytes, &limits).unwrap_err();
    assert!(
        matches!(decode_err, WorldAuditBundleError::OutOfOrderDecisionDelta),
        "expected OutOfOrderDecisionDelta, got: {decode_err:?}"
    );

    // Swap source delta entries out of canonical sorted order
    let mut bundle2 = build_genesis_bundle_from_session(&world, pc, ep).unwrap();
    assert!(bundle2.revisions[0].source_delta.len() >= 2);
    bundle2.revisions[0].source_delta.swap(0, 1);
    let bytes2 = bundle2.encode(&limits).expect("raw encode");
    let decode_err2 = decode_world_audit_bundle_v1(&bytes2, &limits).unwrap_err();
    assert!(
        matches!(decode_err2, WorldAuditBundleError::OutOfOrderSourceDelta),
        "expected OutOfOrderSourceDelta, got: {decode_err2:?}"
    );
}

#[test]
fn t08_single_historical_decision_edit_rejected() {
    let (_dir, mut world, pc, ep) = setup_world("t08");
    world
        .apply_batch(batch(0, vec![upsert("a", "alice", 10)]))
        .unwrap();
    world
        .apply_batch(batch(1, vec![upsert("b", "bob", 20)]))
        .unwrap();

    let mut bundle = build_genesis_bundle_from_session(&world, pc.clone(), ep).unwrap();
    let opts = make_options(bundle.head.revision_digest, pc.program_manifest_digest);

    // Tamper historical revision 1 decision delta without modifying revision 2
    if let Some((_, _, Some(ref mut dt))) = bundle.revisions[0].decision_delta.first_mut() {
        dt.priority = 999;
    }

    let err = verify_world_audit_bundle(&bundle, &opts).unwrap_err();
    assert!(
        matches!(err, VerifyError::DecisionDeltaMismatch { seq: 1 }),
        "expected DecisionDeltaMismatch on historical revision 1, got: {err:?}"
    );
}

#[test]
fn t09_foreign_module_relation_rejected() {
    let (_dir, mut world, pc, ep) = setup_world("t09");
    world
        .apply_batch(batch(0, vec![upsert("a", "alice", 10)]))
        .unwrap();

    let mut bundle = build_genesis_bundle_from_session(&world, pc.clone(), ep).unwrap();
    let opts = make_options(bundle.head.revision_digest, pc.program_manifest_digest);

    // Inject undeclared foreign relation into source delta
    let mut rec = TupleRecord::new();
    rec.set_str("x", "val");
    bundle.revisions[0].source_delta.push((
        "foreign::rogue".to_string(),
        WorldKey::from_str("k"),
        Some(rec.to_tuple()),
    ));
    bundle.revisions[0]
        .source_delta
        .sort_by(|a, b| (a.0.as_str(), a.1.as_bytes()).cmp(&(b.0.as_str(), b.1.as_bytes())));

    let err = verify_world_audit_bundle(&bundle, &opts).unwrap_err();
    assert!(
        matches!(err, VerifyError::Usage(ref msg) if msg.contains("foreign::rogue")),
        "expected unknown relation refusal, got: {err:?}"
    );
}

#[test]
fn t10_honest_replay_without_caches_passes() {
    let (dir, mut world, pc, ep) = setup_world("t10");
    world
        .apply_batch(batch(
            0,
            vec![upsert("a", "alice", 10), upsert("b", "bob", 20)],
        ))
        .unwrap();
    world
        .apply_batch(batch(1, vec![upsert("c", "carol", 30)]))
        .unwrap();

    let bundle = build_genesis_bundle_from_session(&world, pc.clone(), ep).unwrap();
    let head_digest = world.current_revision_digest().unwrap();
    let program_digest = pc.program_manifest_digest;
    let opts = make_options(head_digest, program_digest);

    // Drop the world session and wipe disk directory
    drop(world);
    let _ = fs::remove_dir_all(&dir.0);

    // Verify bundle independently
    let report = verify_world_audit_bundle(&bundle, &opts).unwrap();
    assert_eq!(report.head_revision, 2);
    assert_eq!(report.head_digest, head_digest);
    assert_eq!(report.program_digest, program_digest);
    assert_eq!(report.scope, "complete-from-genesis (0..HEAD)");
    assert!(report.work.tuples_decoded >= 3);
    assert!(report.work.trie_nodes_built >= 3);
    assert!(report.work.index_rows_rebuilt >= 3);
    assert!(report.work.settlements_computed >= 2);
    assert_eq!(report.work.revisions_replayed, 2);
    assert!(!report.verified_settlements.is_empty());
}

#[test]
fn t11_historical_evidence_keeps_original_revision() {
    let (_dir, mut world, pc, ep) = setup_world("t11");
    // Rev 1: a is eligible
    world
        .apply_batch(batch(0, vec![upsert("a", "alice", 10)]))
        .unwrap();
    // Rev 2: a is updated to amount <= 0, so retracted from eligible
    world
        .apply_batch(batch(1, vec![upsert("a", "alice", 0)]))
        .unwrap();

    let bundle = build_genesis_bundle_from_session(&world, pc.clone(), ep).unwrap();
    let opts = make_options(bundle.head.revision_digest, pc.program_manifest_digest);

    // Revision 1 delta has Some decision
    let rev1_has_settlement = bundle.revisions[0]
        .decision_delta
        .iter()
        .any(|(dec, ent, opt)| dec == "root::alpha" && ent == "alice" && opt.is_some());
    assert!(
        rev1_has_settlement,
        "revision 1 must carry settled decision"
    );

    // Revision 2 delta has None decision (retraction)
    let rev2_has_retraction = bundle.revisions[1]
        .decision_delta
        .iter()
        .any(|(dec, ent, opt)| dec == "root::alpha" && ent == "alice" && opt.is_none());
    assert!(
        rev2_has_retraction,
        "revision 2 must carry decision retraction"
    );

    // Both pass verification cleanly
    let report = verify_world_audit_bundle(&bundle, &opts).unwrap();
    assert_eq!(report.head_revision, 2);
}

#[test]
fn t12_emptied_history_is_rejected() {
    let (_dir, mut world, pc, ep) = setup_world("t12");
    world
        .apply_batch(batch(0, vec![upsert("a", "alice", 10)]))
        .unwrap();
    world
        .apply_batch(batch(1, vec![upsert("b", "bob", 20)]))
        .unwrap();

    let honest_bundle = build_genesis_bundle_from_session(&world, pc.clone(), ep).unwrap();
    let real_head_digest = honest_bundle.head.revision_digest;
    let opts = make_options(real_head_digest, pc.program_manifest_digest);

    // Case A: Attacker empties bundle.revisions while keeping head.seq = 2
    let mut empty_bundle = honest_bundle.clone();
    empty_bundle.revisions = vec![];
    let err = verify_world_audit_bundle(&empty_bundle, &opts).unwrap_err();
    assert!(
        matches!(
            err,
            VerifyError::NonContiguousSeq {
                expected: 2,
                found: 0
            }
        ),
        "expected NonContiguousSeq refusing emptied history, got: {err:?}"
    );

    // Case B: Attacker also tampers bundle.head.seq = 0, but honest caller pins expect_head to real HEAD
    let mut forged_head_bundle = empty_bundle.clone();
    forged_head_bundle.head.seq = 0;
    // head.revision_digest is still real_head_digest
    let err_b = verify_world_audit_bundle(&forged_head_bundle, &opts).unwrap_err();
    assert!(
        matches!(err_b, VerifyError::HeadPinMismatch { .. }),
        "expected HeadPinMismatch when seq 0 does not match real head digest, got: {err_b:?}"
    );
}
