//! Adversarial probes for P3. Each test asserts the *correct* behavior; a failure confirms a defect.

use brix_canon::Digest;
use brix_kb::world::{
    CrashPoint, RelationDecl, TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldManifest,
    WorldSession,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brix_p3_adv_{}_{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn manifest() -> WorldManifest {
    WorldManifest::new(
        "adv",
        "2026-10-04T00:00:00Z",
        Digest::of(brix_canon::Domain::Value, b"adv:v1"),
        vec![RelationDecl::new(
            "orders",
            vec!["order_id".to_string()],
            vec!["customer_id".to_string(), "total".to_string()],
            vec!["customer_id".to_string()],
        )],
    )
}

fn up(k: u64, v: &str) -> WorldBatchOp {
    WorldBatchOp::Upsert {
        relation: "orders".into(),
        key: WorldKey::from_u64(k),
        tuple: {
            let mut record = TupleRecord::new();
            record.set_str("customer_id", v);
            record.set_str("total", "0");
            record.to_tuple()
        },
    }
}

#[test]
fn a01_two_stale_writers_must_not_overwrite_history() {
    let dir = test_dir("a01");
    let mut a = WorldSession::create(&dir, manifest()).unwrap();
    a.apply_batch(WorldBatch::new(0, "seed", vec![up(1, "x")]))
        .unwrap();
    let mut b = WorldSession::open(&dir).unwrap();
    a.apply_batch(WorldBatch::new(1, "from-a", vec![up(2, "A")]))
        .unwrap();
    // B is stale (in-memory rev 1, disk rev 2) but nothing checks disk HEAD.
    let r = b.apply_batch(WorldBatch::new(1, "from-b", vec![up(3, "B")]));
    let fresh = WorldSession::open(&dir).unwrap();
    let a_survived = fresh
        .get("orders", &WorldKey::from_u64(2))
        .unwrap()
        .is_some();
    assert!(
        r.is_err() || a_survived,
        "LOST UPDATE: B's commit succeeded={} and A's committed key 2 survived={a_survived}",
        r.is_ok()
    );
}

#[test]
fn a02_idempotency_key_reuse_with_different_payload_must_be_rejected() {
    let dir = test_dir("a02");
    let mut s = WorldSession::create(&dir, manifest()).unwrap();
    s.apply_batch(WorldBatch::new(0, "k", vec![up(1, "first")]))
        .unwrap();
    let r = s.apply_batch(WorldBatch::new(1, "k", vec![up(1, "SECOND-DIFFERENT")]));
    if let Ok(rc) = r {
        assert!(
            !rc.is_idempotent_replay,
            "different payload under same key was silently acked as replay"
        );
    }
}

#[test]
fn a03_genesis_key_must_behave_identically_after_reopen() {
    let dir = test_dir("a03");
    let mut s = WorldSession::create(&dir, manifest()).unwrap();
    let r1 = s
        .apply_batch(WorldBatch::new(0, "genesis", vec![up(1, "x")]))
        .unwrap();
    let dir2 = test_dir("a03b");
    let _s2 = WorldSession::create(&dir2, manifest()).unwrap();
    let mut s2 = WorldSession::open(&dir2).unwrap();
    let r2 = s2
        .apply_batch(WorldBatch::new(0, "genesis", vec![up(1, "x")]))
        .unwrap();
    assert_eq!(
        r1.is_idempotent_replay, r2.is_idempotent_replay,
        "create-session applied (replay={}) but reopened session replayed (replay={}) for same batch",
        r1.is_idempotent_replay, r2.is_idempotent_replay
    );
}

#[test]
fn a04_missing_object_must_be_an_error_not_absent() {
    let dir = test_dir("a04");
    let mut s = WorldSession::create(&dir, manifest()).unwrap();
    s.apply_batch(WorldBatch::new(0, "seed", vec![up(1, "x")]))
        .unwrap();
    s.close().unwrap();
    for sub in fs::read_dir(dir.join("objects")).unwrap().flatten() {
        let path = sub.path();
        if path.is_dir() {
            let _ = fs::remove_dir_all(path);
        } else {
            let _ = fs::remove_file(path);
        }
    }
    let s = WorldSession::open(&dir).unwrap();
    let got = s.get("orders", &WorldKey::from_u64(1));
    assert!(
        got.is_err(),
        "existing key reads as {:?} after its object was deleted (indistinguishable from retracted)",
        got
    );
}

#[test]
fn a05_tampered_object_must_be_detected() {
    let dir = test_dir("a05");
    let mut s = WorldSession::create(&dir, manifest()).unwrap();
    s.apply_batch(WorldBatch::new(0, "seed", vec![up(1, "ORIGINAL")]))
        .unwrap();
    s.close().unwrap();
    let mut tampered_count = 0;
    fn tamper_objects(path: &std::path::Path, tampered_count: &mut usize) {
        if path.is_dir() {
            for entry in fs::read_dir(path).unwrap().flatten() {
                tamper_objects(&entry.path(), tampered_count);
            }
        } else if path.is_file() {
            let mut b = fs::read(path).unwrap();
            if let Some(pos) = b.windows(8).position(|w| w == b"ORIGINAL") {
                b[pos] = b'T'; // TRIGINAL
                fs::write(path, b).unwrap();
                *tampered_count += 1;
            }
        }
    }
    tamper_objects(&dir.join("objects"), &mut tampered_count);
    assert!(
        tampered_count > 0,
        "at least one object must contain ORIGINAL"
    );
    let s = WorldSession::open(&dir).unwrap();
    let got = s.get("orders", &WorldKey::from_u64(1));
    assert!(
        got.is_err(),
        "tampered object content accepted without digest check: {:?}",
        got
    );
}

#[test]
fn a06_corrupt_cardinality_must_not_make_relation_empty() {
    let dir = test_dir("a06");
    let mut s = WorldSession::create(&dir, manifest()).unwrap();
    s.apply_batch(WorldBatch::new(0, "seed", vec![up(1, "x")]))
        .unwrap();
    s.close().unwrap();
    let p = dir.join("revisions").join("1.json");
    let txt = fs::read_to_string(&p).unwrap();
    let txt = txt.replace("\"orders\": 1", "\"orders\": 0");
    fs::write(&p, txt).unwrap();
    let opened = WorldSession::open(&dir);
    let ok = match opened {
        Err(_) => true,
        Ok(s) => s
            .get("orders", &WorldKey::from_u64(1))
            .map(|v| v.is_some())
            .unwrap_or(true),
    };
    assert!(
        ok,
        "edited revision record (digest not recomputed) silently emptied the relation"
    );
}

#[test]
fn a07_commit_staged_zero_chunks_must_not_destroy_staged_data() {
    let dir = test_dir("a07");
    let mut s = WorldSession::create(&dir, manifest()).unwrap();
    let b = WorldBatch::new(0, "c0", vec![up(1, "x")]);
    s.stage_chunk("up1", 0, &b).unwrap();
    let r = s.commit_staged("up1", 0, "commit-0");
    let still = s.paths.staging_upload_dir("up1").exists();
    assert!(
        r.is_err() || still,
        "commit_staged(expected_chunks=0) succeeded and deleted staged chunk 0 (data loss, no manifest)"
    );
}

#[test]
fn a08_staging_traversal_must_not_delete_world() {
    let dir = test_dir("a08");
    let mut s = WorldSession::create(&dir, manifest()).unwrap();
    s.apply_batch(WorldBatch::new(0, "seed", vec![up(1, "x")]))
        .unwrap();
    let _ = s.commit_staged("..", 0, "evil");
    assert!(
        dir.join("world.json").exists() && dir.join("HEAD").exists(),
        "upload_id=\"..\" deleted the world directory (path traversal + remove_dir_all)"
    );
}

#[test]
fn a09_failed_commit_must_not_lose_staged_upload() {
    let dir = test_dir("a09");
    let mut s = WorldSession::create(&dir, manifest()).unwrap();
    let b = WorldBatch::new(0, "c0", vec![up(1, "x")]);
    s.stage_chunk("up1", 0, &b).unwrap();
    s.set_crash_point(Some(CrashPoint::BeforeObjectsFsync));
    assert!(s.commit_staged("up1", 1, "commit-1").is_err());
    assert!(
        s.paths.staging_upload_dir("up1").exists(),
        "staging dir deleted before publication succeeded; retry impossible"
    );
}

#[test]
fn a10_noop_operations_must_not_create_changes() {
    let dir = test_dir("a10");
    let mut s = WorldSession::create(&dir, manifest()).unwrap();
    s.apply_batch(WorldBatch::new(0, "seed", vec![up(1, "same")]))
        .unwrap();
    let rc = s
        .apply_batch(WorldBatch::new(
            1,
            "noop",
            vec![
                up(1, "same"),
                WorldBatchOp::Remove {
                    relation: "orders".into(),
                    key: WorldKey::from_u64(999),
                },
            ],
        ))
        .unwrap();
    let d = s.diff_page("orders", 1, 2, None, 100).unwrap();
    assert!(
        rc.changed_keys_count == 0 && d.events.is_empty(),
        "no-op batch recorded changed_keys_count={} and diff events={}",
        rc.changed_keys_count,
        d.events.len()
    );
}

#[test]
fn a11_diff_page_must_honor_limit_and_cursor() {
    let dir = test_dir("a11");
    let mut s = WorldSession::create(&dir, manifest()).unwrap();
    let ops: Vec<_> = (0..20).map(|i| up(i, "v")).collect();
    s.apply_batch(WorldBatch::new(0, "seed", ops)).unwrap();
    let d = s.diff_page("orders", 0, 1, None, 5).unwrap();
    assert!(
        d.events.len() <= 5 && d.has_more,
        "diff_page(limit=5) returned {} events, has_more={}",
        d.events.len(),
        d.has_more
    );
}

#[test]
fn a12_declared_secondary_index_must_be_maintained() {
    let dir = test_dir("a12");
    let mut s = WorldSession::create(&dir, manifest()).unwrap();
    s.apply_batch(WorldBatch::new(0, "seed", vec![up(1, "x")]))
        .unwrap();
    let rev = s.pin_revision(1).unwrap().revision;
    assert!(
        !rev.secondary_index_roots.is_empty(),
        "manifest declares secondary index orders:customer_id but revision has no index roots"
    );
}

#[test]
fn a13_write_failures_must_not_publish_head() {
    let dir = test_dir("a13");
    let mut s = WorldSession::create(&dir, manifest()).unwrap();
    let obj = dir.join("objects");
    fs::set_permissions(&obj, fs::Permissions::from_mode(0o555)).unwrap();
    let r = s.apply_batch(WorldBatch::new(0, "seed", vec![up(1, "x")]));
    fs::set_permissions(&obj, fs::Permissions::from_mode(0o755)).unwrap();
    let head = fs::read_to_string(dir.join("HEAD")).unwrap();
    let objects_present = fs::read_dir(&obj)
        .unwrap()
        .flatten()
        .any(|e| e.path().is_dir());
    assert!(
        r.is_err() || objects_present,
        "object writes failed silently (ok={}), HEAD published as {:?} with no objects on disk",
        r.is_ok(),
        head.trim()
    );
}

#[test]
fn a14_measure_real_p3_numbers() {
    // Same shape as test_one_key_durable_edit_writes_only_changed_path_nodes, but printed.
    let dir = test_dir("a14");
    let mut s = WorldSession::create(&dir, manifest()).unwrap();
    let pad = "p".repeat(500);
    let ops: Vec<_> = (0..8_000u64)
        .map(|i| up(i, &format!("order_{i}{pad}")))
        .collect();
    s.apply_batch(WorldBatch::new(0, "seed", ops)).unwrap();
    let (b0, n0) = (s.node_store.total_bytes(), s.node_store.node_count());
    s.apply_batch(WorldBatch::new(1, "edit-warm", vec![up(4242, "warm-edit")]))
        .unwrap();
    let (b1, n1) = (s.node_store.total_bytes(), s.node_store.node_count());
    let rev_len = fs::metadata(dir.join("revisions").join("2.json"))
        .unwrap()
        .len();
    s.close().unwrap();
    let mut cold = WorldSession::open(&dir).unwrap();
    cold.apply_batch(WorldBatch::new(2, "edit-cold", vec![up(1234, "cold-edit")]))
        .unwrap();
    let (b2, n2) = (cold.node_store.total_bytes(), cold.node_store.node_count());
    println!("P3-MEASURED seed: objects={n0} bytes={b0}");
    println!(
        "P3-MEASURED warm 1-key edit: +{} objects, +{} bytes, revision record {} bytes",
        n1 - n0,
        b1 - b0,
        rev_len
    );
    println!(
        "P3-MEASURED cold 1-key edit: +{} objects, +{} bytes",
        n2 - n1,
        b2 - b1
    );
}
