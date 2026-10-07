//! Tests for active verification resource limits and budget enforcement (P6).
//!
//! Validates:
//! 1. Early abort on oversized tuple decodes before unbounded work occurs.
//! 2. Early abort inside EquiJoin evaluation loops under constrained work budget.
//! 3. Early abort during checkpoint state reconstruction under constrained budget.
//! 4. Honest audit verification passes with adequate budget and records accurate work meters.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use brix_canon::{Digest, Domain};
use brix_kb::world::audit::{
    build_checkpoint_bundle_from_session, build_genesis_bundle_from_session, ExecProfileV1,
    ProgramClosureV1, WorldAuditDecodeLimits,
};
use brix_kb::world::verify::{verify_world_audit_bundle, VerifyError, VerifyOptions};
use brix_kb::world::{
    RelationDecl, TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldManifest, WorldSession,
};

fn temp_test_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("brix_verify_budget_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create test dir");
    dir
}

fn upsert_row(rel: &str, id: &str, fields: &[(&str, &str)]) -> WorldBatchOp {
    let mut tuple = TupleRecord::new();
    tuple.set_str("id", id);
    for (k, v) in fields {
        tuple.set_str(*k, v);
    }
    WorldBatchOp::Upsert {
        relation: rel.into(),
        key: WorldKey::from_str(id),
        tuple: tuple.to_tuple(),
    }
}

#[test]
fn test_early_budget_refusal_on_oversized_tuple_expansion() {
    let dir = temp_test_dir("tuple_expansion");
    let manifest = WorldManifest::new(
        "budget-test",
        "2026-10-06T00:00:00Z",
        Digest::of(Domain::Value, b"initial"),
        vec![RelationDecl::new(
            "root::items",
            vec!["id".into()],
            vec!["name".into(), "val".into()],
            vec![],
        )],
    );
    let mut session = WorldSession::create(&dir, manifest).unwrap();
    let src = "rel input items: { id: Str, name: Str, val: Str } key id\n";
    let sources = BTreeMap::from([("root".into(), src.into())]);
    session.save_program_closure("root", &sources).unwrap();

    let pc = ProgramClosureV1 {
        root_module: "root".to_string(),
        sources: vec![("root".to_string(), src.to_string())],
        program_manifest_digest: session.manifest().program_digest,
    };
    let ep = ExecProfileV1::default();

    // Ingest 500 rows across several revisions
    for rev in 0..5 {
        let mut ops = Vec::new();
        for i in 0..100 {
            let id = format!("item_{}_{}", rev, i);
            ops.push(upsert_row(
                "root::items",
                &id,
                &[("name", "widget"), ("val", "42")],
            ));
        }
        session
            .apply_batch(WorldBatch::new(
                session.current_revision(),
                format!("batch_{rev}"),
                ops,
            ))
            .expect("apply batch");
    }

    let bundle = build_genesis_bundle_from_session(&session, pc.clone(), ep).expect("build bundle");

    // With a tiny max_work of 20, verification must abort early during tuple decoding
    let options = VerifyOptions {
        expect_program: pc.program_manifest_digest,
        expect_head: bundle.head.revision_digest,
        trust_checkpoint: None,
        limits: WorldAuditDecodeLimits::default(),
        max_work: Some(20),
    };

    let result = verify_world_audit_bundle(&bundle, &options);
    match result {
        Err(VerifyError::BudgetExhausted) => {
            // Success: safely refused early due to budget exhaustion
        }
        other => panic!("expected VerifyError::BudgetExhausted, got {other:?}"),
    }
}

#[test]
fn test_early_budget_refusal_on_oversized_equijoin() {
    let dir = temp_test_dir("equijoin");
    let manifest = WorldManifest::new(
        "budget-join",
        "2026-10-06T00:00:00Z",
        Digest::of(Domain::Value, b"initial"),
        vec![
            RelationDecl::new(
                "root::left_items",
                vec!["id".into()],
                vec!["tag".into()],
                vec![],
            ),
            RelationDecl::new(
                "root::right_items",
                vec!["id".into()],
                vec!["tag".into()],
                vec![],
            ),
        ],
    );
    let mut session = WorldSession::create(&dir, manifest).unwrap();

    let src = r#"
rel input left_items: { id: Str, tag: Str } key id
rel input right_items: { id: Str, tag: Str } key id

rel derived joined =
    select { l_id: l.id, r_id: r.id, tag: l.tag }
    from l in left_items, r in right_items
    where l.tag == r.tag

decide matches for j in joined per l_id {
    propose matched priority 10 when j.tag == "common" = "ok"
}
"#;
    let sources = BTreeMap::from([("root".into(), src.into())]);
    session.save_program_closure("root", &sources).unwrap();

    let pc = ProgramClosureV1 {
        root_module: "root".to_string(),
        sources: vec![("root".to_string(), src.to_string())],
        program_manifest_digest: session.manifest().program_digest,
    };
    let ep = ExecProfileV1::default();

    // Ingest 150 left items and 150 right items all with matching tag
    // Potential Cartesian product is 150 * 150 = 22,500 pairs
    let mut ops = Vec::new();
    for i in 0..150 {
        ops.push(upsert_row(
            "root::left_items",
            &format!("L_{i}"),
            &[("tag", "common")],
        ));
        ops.push(upsert_row(
            "root::right_items",
            &format!("R_{i}"),
            &[("tag", "common")],
        ));
    }
    session
        .apply_batch(WorldBatch::new(
            session.current_revision(),
            "batch_join",
            ops,
        ))
        .expect("apply batch");

    let bundle = build_genesis_bundle_from_session(&session, pc.clone(), ep).expect("build bundle");

    // max_work of 350 allows initial tuples (300) to decode, but cuts off the join
    // loops during reference evaluation before completing 22,500 join pairs!
    let options = VerifyOptions {
        expect_program: pc.program_manifest_digest,
        expect_head: bundle.head.revision_digest,
        trust_checkpoint: None,
        limits: WorldAuditDecodeLimits::default(),
        max_work: Some(350),
    };

    let result = verify_world_audit_bundle(&bundle, &options);
    match result {
        Err(VerifyError::BudgetExhausted) => {
            // Success: halted early during join expansion
        }
        other => panic!("expected VerifyError::BudgetExhausted, got {other:?}"),
    }
}

#[test]
fn test_early_budget_refusal_on_checkpoint_reconstruction() {
    let dir = temp_test_dir("checkpoint_reconstruction");
    let manifest = WorldManifest::new(
        "budget-ckpt",
        "2026-10-06T00:00:00Z",
        Digest::of(Domain::Value, b"initial"),
        vec![RelationDecl::new(
            "root::records",
            vec!["id".into()],
            vec!["data".into()],
            vec![],
        )],
    );
    let mut session = WorldSession::create(&dir, manifest).unwrap();

    let src = "rel input records: { id: Str, data: Str } key id\n";
    let sources = BTreeMap::from([("root".into(), src.into())]);
    session.save_program_closure("root", &sources).unwrap();

    let pc = ProgramClosureV1 {
        root_module: "root".to_string(),
        sources: vec![("root".to_string(), src.to_string())],
        program_manifest_digest: session.manifest().program_digest,
    };
    let ep = ExecProfileV1::default();

    let mut ops = Vec::new();
    for i in 0..300 {
        ops.push(upsert_row(
            "root::records",
            &format!("rec_{i}"),
            &[("data", "value")],
        ));
    }
    session
        .apply_batch(WorldBatch::new(
            session.current_revision(),
            "batch_ckpt",
            ops,
        ))
        .expect("apply batch");

    let cp_bundle = build_checkpoint_bundle_from_session(&session, 1, pc.clone(), ep)
        .expect("build checkpoint bundle");

    // Small budget should abort during checkpoint tuple decoding
    let options = VerifyOptions {
        expect_program: pc.program_manifest_digest,
        expect_head: cp_bundle.head.revision_digest,
        trust_checkpoint: Some(cp_bundle.head.revision_digest),
        limits: WorldAuditDecodeLimits::default(),
        max_work: Some(25),
    };

    let result = verify_world_audit_bundle(&cp_bundle, &options);
    match result {
        Err(VerifyError::BudgetExhausted) => {
            // Success: stopped during checkpoint relation decoding
        }
        other => panic!("expected VerifyError::BudgetExhausted, got {other:?}"),
    }
}

#[test]
fn test_honest_bundle_succeeds_with_adequate_budget() {
    let dir = temp_test_dir("honest_adequate");
    let src = r#"
rel input inventory: { id: Str, qty: Str } key id
rel derived stock = select { id: i.id, qty: i.qty } from i in inventory
decide restock for s in stock per id {
    propose check priority 10 when true = "in_stock"
}
"#;
    let sources = BTreeMap::from([("root".into(), src.into())]);
    let mut session = WorldSession::from_program_with_sources(&dir, "root", &sources).unwrap();

    let pc = ProgramClosureV1 {
        root_module: "root".to_string(),
        sources: vec![("root".to_string(), src.to_string())],
        program_manifest_digest: session.manifest().program_digest,
    };
    let ep = ExecProfileV1::default();

    let mut ops = Vec::new();
    for i in 0..20 {
        ops.push(upsert_row(
            "root::inventory",
            &format!("inv_{i}"),
            &[("qty", "100")],
        ));
    }
    session
        .apply_batch(WorldBatch::new(
            session.current_revision(),
            "batch_honest",
            ops,
        ))
        .expect("apply batch");

    let bundle = build_genesis_bundle_from_session(&session, pc.clone(), ep).expect("build bundle");

    let options = VerifyOptions {
        expect_program: pc.program_manifest_digest,
        expect_head: bundle.head.revision_digest,
        trust_checkpoint: None,
        limits: WorldAuditDecodeLimits::default(),
        max_work: Some(50_000),
    };

    let report = verify_world_audit_bundle(&bundle, &options)
        .expect("honest verification must pass with adequate budget");

    assert!(report.work.tuples_decoded > 0);
    assert!(report.work.trie_nodes_built > 0);
    assert!(report.work.revisions_replayed > 0);
    assert!(report.work.total_work() < 50_000);
}
