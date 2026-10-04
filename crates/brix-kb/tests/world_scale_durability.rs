//! Scale, durability, and crash-resilience qualification tests for Stage P3 (ADR-0046).

use brix_canon::Digest;
use brix_kb::world::{
    CrashPoint, DiffEvent, RelationDecl, TupleRecord, WorldBatch, WorldBatchOp, WorldError,
    WorldKey, WorldManifest, WorldOracle, WorldSession, WorldTuple,
};
use std::fs;
use std::path::PathBuf;

fn order_tuple(payload: &str) -> WorldTuple {
    let mut record = TupleRecord::new();
    record.set_str("customer_id", "customer_fixture");
    record.set_str("total", "0");
    record.set_str("status", payload);
    record.to_tuple()
}

fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brix_world_p3_{}_{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn sample_manifest() -> WorldManifest {
    WorldManifest::new(
        "world-test-01",
        "2026-10-04T00:00:00Z",
        Digest::of(brix_canon::Domain::Value, b"test:program:v1"),
        vec![
            RelationDecl::new(
                "orders",
                vec!["order_id".to_string()],
                vec![
                    "customer_id".to_string(),
                    "total".to_string(),
                    "status".to_string(),
                ],
                vec!["customer_id".to_string()],
            ),
            RelationDecl::new(
                "inventory",
                vec!["item_id".to_string()],
                vec![
                    "sku".to_string(),
                    "qty".to_string(),
                    "warehouse".to_string(),
                ],
                vec![],
            ),
            RelationDecl::new(
                "audit",
                vec!["event_id".to_string()],
                vec!["payload".to_string()],
                vec![],
            ),
        ],
    )
}

#[test]
fn test_world_exceeding_4mib_survival_and_lazy_cold_reopen() {
    let dir = test_dir("4mib_survival");
    let manifest = sample_manifest();
    let mut oracle = WorldOracle::new(manifest.clone());
    let mut session = WorldSession::create(&dir, manifest).expect("create world session");

    // Ingest 8,000 orders + 2,000 inventory items with ~500-byte payloads to exceed 4 MiB
    let payload_padding = vec![0x42u8; 480];
    let mut batch_ops = Vec::with_capacity(10_000);

    for i in 0..8_000u64 {
        let key = WorldKey::from_u64(i);
        let mut record = TupleRecord::new();
        record.set_str("customer_id", &format!("customer_{:05}", i % 100));
        record.set_str("total", &(i * 10).to_string());
        record.set_str("status", "pending");
        record.set("padding", payload_padding.clone());
        let tuple = record.to_tuple();

        batch_ops.push(WorldBatchOp::Upsert {
            relation: "orders".to_string(),
            key,
            tuple,
        });
    }

    for i in 0..2_000u64 {
        let key = WorldKey::from_u64(i);
        let mut tuple_bytes = format!("sku_{:06}:qty_{}:warehouse_main:", i, i + 50).into_bytes();
        tuple_bytes.extend_from_slice(&payload_padding);
        let tuple = WorldTuple::new(tuple_bytes);

        batch_ops.push(WorldBatchOp::Upsert {
            relation: "inventory".to_string(),
            key,
            tuple,
        });
    }

    let batch = WorldBatch::new(0, "batch-initial-large-load", batch_ops);
    oracle.apply_batch(&batch).expect("oracle apply batch");

    let receipt = session.apply_batch(batch).expect("session apply batch");
    assert_eq!(receipt.revision_seq, 1);
    assert_eq!(receipt.changed_keys_count, 10_000);
    assert!(receipt.objects_written > 0);

    // Verify stored objects size strictly exceeds 4 MiB (4 * 1024 * 1024 bytes = 4,194,304 bytes)
    let total_stored_bytes = session.node_store.total_bytes();
    assert!(
        total_stored_bytes > 4 * 1024 * 1024,
        "world objects size must exceed 4 MiB gate, got: {total_stored_bytes} bytes"
    );

    // Apply a second batch with modifications and retractions
    let mut update_ops = Vec::new();
    // Update order 100
    update_ops.push(WorldBatchOp::Upsert {
        relation: "orders".to_string(),
        key: WorldKey::from_u64(100),
        tuple: order_tuple("customer_00000:total_99999:status_completed"),
    });
    // Remove order 200
    update_ops.push(WorldBatchOp::Remove {
        relation: "orders".to_string(),
        key: WorldKey::from_u64(200),
    });

    let batch2 = WorldBatch::new(1, "batch-updates-and-retracts", update_ops);
    oracle.apply_batch(&batch2).expect("oracle apply batch2");
    let receipt2 = session.apply_batch(batch2).expect("session apply batch2");
    assert_eq!(receipt2.revision_seq, 2);

    // Close session and flush all buffers
    session.close().expect("close session");

    // Cold reopen from disk
    let cold_session = WorldSession::open(&dir).expect("reopen world session");
    assert_eq!(cold_session.current_revision(), 2);

    // Verify lazy lookups on cold session
    let ord100 = cold_session
        .get("orders", &WorldKey::from_u64(100))
        .expect("get ord100")
        .expect("ord100 exists");
    assert_eq!(
        ord100,
        *oracle.get("orders", &WorldKey::from_u64(100)).unwrap()
    );

    let ord200 = cold_session
        .get("orders", &WorldKey::from_u64(200))
        .expect("get ord200");
    assert_eq!(ord200, None, "order 200 was retracted and must be None");

    let ord500 = cold_session
        .get("orders", &WorldKey::from_u64(500))
        .expect("get ord500")
        .expect("ord500 exists");
    assert_eq!(
        ord500,
        *oracle.get("orders", &WorldKey::from_u64(500)).unwrap()
    );

    // Non-existent key
    let missing = cold_session
        .get("orders", &WorldKey::from_u64(999_999))
        .expect("get missing");
    assert_eq!(missing, None);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_one_key_durable_edit_writes_only_changed_path_nodes() {
    let dir = test_dir("one_key_minimal_io");
    let manifest = sample_manifest();
    let mut session = WorldSession::create(&dir, manifest).expect("create world session");

    // Ingest 8,000 items with padding so the world size > 4 MiB into unindexed relation 'inventory'
    let payload_padding = vec![0xaa; 500];
    let mut initial_ops = Vec::with_capacity(8_000);
    for i in 0..8_000u64 {
        let mut val = format!("sku_{i}:warehouse_01:qty_100").into_bytes();
        val.extend_from_slice(&payload_padding);
        initial_ops.push(WorldBatchOp::Upsert {
            relation: "inventory".to_string(),
            key: WorldKey::from_u64(i),
            tuple: WorldTuple::new(val),
        });
    }

    session
        .apply_batch(WorldBatch::new(0, "batch-large-seed", initial_ops))
        .expect("seed batch");

    let before_bytes = session.node_store.total_bytes();
    let before_count = session.node_store.node_count();
    assert!(
        before_bytes > 4 * 1024 * 1024,
        "seed must exceed 4 MiB, got {before_bytes} bytes"
    );

    // Apply a 1-key durable edit to unindexed relation
    let one_key_batch = WorldBatch::new(
        1,
        "batch-one-key-edit",
        vec![WorldBatchOp::Upsert {
            relation: "inventory".to_string(),
            key: WorldKey::from_u64(4242),
            tuple: WorldTuple::from_str("sku_4242:warehouse_01:qty_999_updated"),
        }],
    );

    let receipt = session
        .apply_batch(one_key_batch)
        .expect("apply one key edit");
    assert_eq!(receipt.revision_seq, 2);
    assert_eq!(receipt.changed_keys_count, 1);

    let after_bytes = session.node_store.total_bytes();
    let after_count = session.node_store.node_count();

    let new_objects = after_count - before_count;
    let new_bytes = after_bytes - before_bytes;

    // A single key edit in a persistent 16-way HAMT with 8,000 keys traverses depth <= 6.
    // It must allocate only O(log n) path nodes (at most ~6 nodes), NEVER rewriting the whole world.
    assert!(
        new_objects <= 8,
        "1-key edit wrote {new_objects} new objects; expected <= 8 path nodes"
    );
    assert!(
        new_bytes < 10 * 1024,
        "1-key edit wrote {new_bytes} bytes; expected compact delta < 10 KiB, not full-world copy"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_crash_injection_at_persistence_boundaries() {
    let dir = test_dir("crash_injection");
    let manifest = sample_manifest();
    let mut session = WorldSession::create(&dir, manifest).expect("create world session");

    // Seed revision 1
    session
        .apply_batch(WorldBatch::new(
            0,
            "seed-rev1",
            vec![WorldBatchOp::Upsert {
                relation: "orders".to_string(),
                key: WorldKey::from_u64(1),
                tuple: order_tuple("seed_order"),
            }],
        ))
        .expect("seed rev 1");
    assert_eq!(session.current_revision(), 1);

    // Test 1: Crash before objects fsync
    session.set_crash_point(Some(CrashPoint::BeforeObjectsFsync));
    let err1 = session
        .apply_batch(WorldBatch::new(
            1,
            "attempt-crash-1",
            vec![WorldBatchOp::Upsert {
                relation: "orders".to_string(),
                key: WorldKey::from_u64(2),
                tuple: order_tuple("crash_order_1"),
            }],
        ))
        .unwrap_err();
    assert!(matches!(
        err1,
        WorldError::InjectedCrash(CrashPoint::BeforeObjectsFsync)
    ));

    // Reopen after crash: must see revision 1 intact
    let mut session = WorldSession::open(&dir).expect("reopen 1");
    assert_eq!(session.current_revision(), 1);
    assert_eq!(session.get("orders", &WorldKey::from_u64(2)).unwrap(), None);

    // Retry the attempt: must succeed cleanly
    session
        .apply_batch(WorldBatch::new(
            1,
            "attempt-crash-1",
            vec![WorldBatchOp::Upsert {
                relation: "orders".to_string(),
                key: WorldKey::from_u64(2),
                tuple: order_tuple("crash_order_1"),
            }],
        ))
        .expect("retry after crash 1");
    assert_eq!(session.current_revision(), 2);

    // Test 2: Crash after objects fsync before revision fsync
    session.set_crash_point(Some(CrashPoint::AfterObjectsFsyncBeforeRevisionFsync));
    let err2 = session
        .apply_batch(WorldBatch::new(
            2,
            "attempt-crash-2",
            vec![WorldBatchOp::Upsert {
                relation: "orders".to_string(),
                key: WorldKey::from_u64(3),
                tuple: order_tuple("crash_order_2"),
            }],
        ))
        .unwrap_err();
    assert!(matches!(
        err2,
        WorldError::InjectedCrash(CrashPoint::AfterObjectsFsyncBeforeRevisionFsync)
    ));

    // Reopen: HEAD still at revision 2
    let mut session = WorldSession::open(&dir).expect("reopen 2");
    assert_eq!(session.current_revision(), 2);

    // Retry: succeeds idempotently, deduplicating stored objects
    session
        .apply_batch(WorldBatch::new(
            2,
            "attempt-crash-2",
            vec![WorldBatchOp::Upsert {
                relation: "orders".to_string(),
                key: WorldKey::from_u64(3),
                tuple: order_tuple("crash_order_2"),
            }],
        ))
        .expect("retry after crash 2");
    assert_eq!(session.current_revision(), 3);

    // Test 3: Crash after revision fsync before HEAD rename
    session.set_crash_point(Some(CrashPoint::AfterRevisionFsyncBeforeHeadRename));
    let err3 = session
        .apply_batch(WorldBatch::new(
            3,
            "attempt-crash-3",
            vec![WorldBatchOp::Upsert {
                relation: "orders".to_string(),
                key: WorldKey::from_u64(4),
                tuple: order_tuple("crash_order_3"),
            }],
        ))
        .unwrap_err();
    assert!(matches!(
        err3,
        WorldError::InjectedCrash(CrashPoint::AfterRevisionFsyncBeforeHeadRename)
    ));

    // Reopen: HEAD is still 3 because atomic rename did not happen
    let mut session = WorldSession::open(&dir).expect("reopen 3");
    assert_eq!(session.current_revision(), 3);

    // Retry attempt 3: succeeds and publishes HEAD
    session
        .apply_batch(WorldBatch::new(
            3,
            "attempt-crash-3",
            vec![WorldBatchOp::Upsert {
                relation: "orders".to_string(),
                key: WorldKey::from_u64(4),
                tuple: order_tuple("crash_order_3"),
            }],
        ))
        .expect("retry after crash 3");
    assert_eq!(session.current_revision(), 4);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_idempotent_batch_replay() {
    let dir = test_dir("idempotency");
    let manifest = sample_manifest();
    let mut session = WorldSession::create(&dir, manifest).expect("create world session");

    let batch = WorldBatch::new(
        0,
        "unique-idempotency-key-xyz",
        vec![WorldBatchOp::Upsert {
            relation: "orders".to_string(),
            key: WorldKey::from_u64(777),
            tuple: order_tuple("order_777_payload"),
        }],
    );

    let receipt1 = session.apply_batch(batch.clone()).expect("apply batch 1");
    assert_eq!(receipt1.revision_seq, 1);
    assert!(!receipt1.is_idempotent_replay);

    // Re-submitting the same batch with identical idempotency key
    let receipt2 = session.apply_batch(batch).expect("apply batch replay");
    assert_eq!(receipt2.revision_seq, 1);
    assert_eq!(receipt2.revision_digest, receipt1.revision_digest);
    assert!(receipt2.is_idempotent_replay);
    assert_eq!(
        session.current_revision(),
        1,
        "revision seq must not advance on idempotent replay"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_batch_conflict_and_stale_base_rejections() {
    let dir = test_dir("conflicts");
    let manifest = sample_manifest();
    let mut session = WorldSession::create(&dir, manifest).expect("create world session");

    // 1. Contradictory upserts in same batch
    let conflicting_batch1 = WorldBatch::new(
        0,
        "bad-batch-1",
        vec![
            WorldBatchOp::Upsert {
                relation: "orders".to_string(),
                key: WorldKey::from_u64(10),
                tuple: order_tuple("tuple_alpha"),
            },
            WorldBatchOp::Upsert {
                relation: "orders".to_string(),
                key: WorldKey::from_u64(10),
                tuple: order_tuple("tuple_beta"),
            },
        ],
    );
    let err1 = session.apply_batch(conflicting_batch1).unwrap_err();
    assert!(matches!(err1, WorldError::BatchConflict { .. }));

    // 2. Contradictory upsert and remove in same batch
    let conflicting_batch2 = WorldBatch::new(
        0,
        "bad-batch-2",
        vec![
            WorldBatchOp::Upsert {
                relation: "orders".to_string(),
                key: WorldKey::from_u64(20),
                tuple: order_tuple("tuple_val"),
            },
            WorldBatchOp::Remove {
                relation: "orders".to_string(),
                key: WorldKey::from_u64(20),
            },
        ],
    );
    let err2 = session.apply_batch(conflicting_batch2).unwrap_err();
    assert!(matches!(err2, WorldError::BatchConflict { .. }));

    // 3. Stale base revision rejection
    let stale_batch = WorldBatch::new(
        999, // current is 0
        "stale-batch",
        vec![WorldBatchOp::Upsert {
            relation: "orders".to_string(),
            key: WorldKey::from_u64(30),
            tuple: order_tuple("tuple_val"),
        }],
    );
    let err3 = session.apply_batch(stale_batch).unwrap_err();
    assert!(matches!(
        err3,
        WorldError::StaleBaseRevision {
            expected: 999,
            current: 0
        }
    ));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_staging_chunked_ingestion_and_atomic_commit() {
    let dir = test_dir("staging_chunks");
    let manifest = sample_manifest();
    let mut session = WorldSession::create(&dir, manifest).expect("create world session");

    let upload_id = "bulk-upload-test-001";

    // Stage 3 chunks
    for chunk_seq in 0..3u64 {
        let mut ops = Vec::new();
        for i in 0..50u64 {
            let item_id = chunk_seq * 50 + i;
            ops.push(WorldBatchOp::Upsert {
                relation: "inventory".to_string(),
                key: WorldKey::from_u64(item_id),
                tuple: order_tuple(&format!("item_{item_id}_chunk_{chunk_seq}")),
            });
        }
        let chunk_batch = WorldBatch::new(0, format!("chunk-{chunk_seq}"), ops);
        session
            .stage_chunk(upload_id, chunk_seq, &chunk_batch)
            .expect("stage chunk");
    }

    // Verify staged chunks are NOT visible in the active world
    assert_eq!(session.current_revision(), 0);
    assert_eq!(
        session.get("inventory", &WorldKey::from_u64(10)).unwrap(),
        None
    );

    // Atomically commit staged upload
    let receipt = session
        .commit_staged(upload_id, 3, "bulk-commit-idempotency")
        .expect("commit staged");

    assert_eq!(receipt.revision_seq, 1);
    assert_eq!(receipt.changed_keys_count, 150);

    // Staging directory was cleaned up
    assert!(!session.paths.staging_upload_dir(upload_id).exists());

    // All items across all chunks are now queryable
    for i in 0..150u64 {
        let tuple = session.get("inventory", &WorldKey::from_u64(i)).unwrap();
        assert!(tuple.is_some(), "item {i} must exist after commit");
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_paged_results_and_diffs() {
    let dir = test_dir("paged_diffs");
    let manifest = sample_manifest();
    let mut session = WorldSession::create(&dir, manifest).expect("create world session");

    // Rev 1: insert 10 items
    let mut ops1 = Vec::new();
    for i in 0..10u64 {
        ops1.push(WorldBatchOp::Upsert {
            relation: "orders".to_string(),
            key: WorldKey::from_u64(i),
            tuple: order_tuple(&format!("val_{i}_v1")),
        });
    }
    session
        .apply_batch(WorldBatch::new(0, "batch-paged-seed", ops1))
        .expect("batch 1");

    // Paged query: limit 4
    let page1 = session.query_page("orders", None, 4).expect("page 1");
    assert_eq!(page1.entries.len(), 4);
    assert!(page1.has_more);
    assert!(page1.next_cursor.is_some());

    let page2 = session
        .query_page("orders", page1.next_cursor.as_deref(), 4)
        .expect("page 2");
    assert_eq!(page2.entries.len(), 4);
    assert!(page2.has_more);

    let page3 = session
        .query_page("orders", page2.next_cursor.as_deref(), 4)
        .expect("page 3");
    assert_eq!(page3.entries.len(), 2);
    assert!(!page3.has_more);

    // Rev 2: update item 2, remove item 5, add item 10
    session
        .apply_batch(WorldBatch::new(
            1,
            "batch-diff-changes",
            vec![
                WorldBatchOp::Upsert {
                    relation: "orders".to_string(),
                    key: WorldKey::from_u64(2),
                    tuple: order_tuple("val_2_MODIFIED"),
                },
                WorldBatchOp::Remove {
                    relation: "orders".to_string(),
                    key: WorldKey::from_u64(5),
                },
                WorldBatchOp::Upsert {
                    relation: "orders".to_string(),
                    key: WorldKey::from_u64(10),
                    tuple: order_tuple("val_10_NEW"),
                },
            ],
        ))
        .expect("batch 2");

    // Diff page between rev 1 and rev 2
    let diff = session
        .diff_page("orders", 1, 2, None, 10)
        .expect("diff page");
    assert_eq!(diff.events.len(), 3);

    let has_up2 = diff
        .events
        .iter()
        .any(|e| matches!(e, DiffEvent::Upserted { key, .. } if *key == WorldKey::from_u64(2)));
    let has_rem5 = diff
        .events
        .iter()
        .any(|e| matches!(e, DiffEvent::Removed { key } if *key == WorldKey::from_u64(5)));
    let has_up10 = diff
        .events
        .iter()
        .any(|e| matches!(e, DiffEvent::Upserted { key, .. } if *key == WorldKey::from_u64(10)));

    assert!(has_up2, "diff must contain upsert of key 2");
    assert!(has_rem5, "diff must contain removal of key 5");
    assert!(has_up10, "diff must contain upsert of key 10");

    let _ = fs::remove_dir_all(&dir);
}
