use std::fs;
use std::path::PathBuf;

use brix_canon::{Digest, Domain};
use brix_kb::world::{
    RelationDecl, TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldManifest, WorldSession,
};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("review_retention_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn manifest() -> WorldManifest {
    WorldManifest::new(
        "retention-repro",
        "2026-10-07T00:00:00Z",
        Digest::of(Domain::Value, b"initial"),
        vec![RelationDecl::new(
            "root::logs",
            vec!["id".into()],
            vec!["msg".into()],
            vec![],
        )],
    )
}

fn batch(expected_base: u64, key: &str) -> WorldBatch {
    let mut tuple = TupleRecord::new();
    tuple.set_str("id", "row1");
    tuple.set_str("msg", "hello");
    WorldBatch::new(
        expected_base,
        key,
        vec![WorldBatchOp::Upsert {
            relation: "root::logs".into(),
            key: WorldKey::from_str("row1"),
            tuple: tuple.to_tuple(),
        }],
    )
}

#[test]
fn checkpoint_pin_survives_reopen_and_prevents_compaction() {
    let dir = temp_dir("checkpoint_reopen");
    let mut session = WorldSession::create(&dir, manifest()).unwrap();
    session.apply_batch(batch(0, "first")).unwrap();
    session.pin_checkpoint(1).unwrap();
    session.apply_batch(batch(1, "second")).unwrap();
    drop(session);

    let mut reopened = WorldSession::open(&dir).unwrap();
    assert!(
        reopened.is_revision_pinned(1),
        "checkpoint pin must survive reopen"
    );
    let report = reopened.compact_history(1).unwrap();
    assert_eq!(
        report.revisions_reclaimed, 0,
        "pinned checkpoint must not be reclaimed"
    );
    assert!(reopened.pin_revision(1).is_ok());

    // Once unpinned, compaction can reclaim revision 1
    assert!(reopened.unpin_checkpoint(1));
    // Drop reader pin from above
    drop(reopened);
    let mut reopened2 = WorldSession::open(&dir).unwrap();
    assert!(
        !reopened2.is_revision_pinned(1),
        "unpinned checkpoint must not remain pinned"
    );
    let report2 = reopened2.compact_history(1).unwrap();
    assert_eq!(report2.revisions_reclaimed, 1);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn compaction_preserves_replay_receipt_when_reopened() {
    let dir = temp_dir("idempotency_reopen");
    let mut session = WorldSession::create(&dir, manifest()).unwrap();
    let original = batch(0, "same-key");
    let first = session.apply_batch(original.clone()).unwrap();
    assert!(!first.is_idempotent_replay);
    session.apply_batch(batch(1, "later-key")).unwrap();
    session.compact_history(1).unwrap();
    drop(session);

    let mut reopened = WorldSession::open(&dir).unwrap();
    let replay = reopened
        .apply_batch(original)
        .expect("compaction must preserve idempotent replay receipts across reopen");
    assert!(replay.is_idempotent_replay);
    assert_eq!(replay.revision_seq, first.revision_seq);
    assert_eq!(replay.revision_digest, first.revision_digest);
    let _ = fs::remove_dir_all(&dir);
}
