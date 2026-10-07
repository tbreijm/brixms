//! Rigorous qualification tests for secondary index storage contract,
//! path-bounded incremental updates, and query invariants (ADR-0046, Stage P3).

use brix_canon::Digest;
use brix_kb::world::{
    encode_secondary_key, RelationDecl, TupleRecord, WorldBatch, WorldBatchOp, WorldError,
    WorldKey, WorldManifest, WorldSession, WorldTuple,
};
use std::fs;
use std::path::PathBuf;

fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brix_sec_idx_{}_{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn indexed_manifest() -> WorldManifest {
    WorldManifest::new(
        "test-sec-idx",
        "2026-10-04T00:00:00Z",
        Digest::of(brix_canon::Domain::Value, b"test:sec:v1"),
        vec![
            // 1 index
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
            // 0 indexes
            RelationDecl::new(
                "inventory",
                vec!["item_id".to_string()],
                vec!["sku".to_string(), "qty".to_string()],
                vec![],
            ),
            // 2 indexes
            RelationDecl::new(
                "shipments",
                vec!["shipment_id".to_string()],
                vec![
                    "customer_id".to_string(),
                    "carrier".to_string(),
                    "status".to_string(),
                ],
                vec!["customer_id".to_string(), "carrier".to_string()],
            ),
        ],
    )
}

fn make_order_tuple(customer: &str, total: u64, status: &str) -> WorldTuple {
    let mut rec = TupleRecord::new();
    rec.set_str("customer_id", customer);
    rec.set_str("total", &total.to_string());
    rec.set_str("status", status);
    rec.to_tuple()
}

fn make_shipment_tuple(customer: &str, carrier: &str, status: &str) -> WorldTuple {
    let mut rec = TupleRecord::new();
    rec.set_str("customer_id", customer);
    rec.set_str("carrier", carrier);
    rec.set_str("status", status);
    rec.to_tuple()
}

#[test]
fn test_secondary_index_six_ruling_invariants() {
    let dir = test_dir("six_invariants");
    let manifest = indexed_manifest();
    let mut session = WorldSession::create(&dir, manifest).expect("create world");

    // 1. Two orders for one customer are both returned
    let batch1 = WorldBatch::new(
        0,
        "seed-two-orders",
        vec![
            WorldBatchOp::Upsert {
                relation: "orders".to_string(),
                key: WorldKey::from_u64(101),
                tuple: make_order_tuple("alice", 50, "pending"),
            },
            WorldBatchOp::Upsert {
                relation: "orders".to_string(),
                key: WorldKey::from_u64(102),
                tuple: make_order_tuple("alice", 150, "pending"),
            },
            WorldBatchOp::Upsert {
                relation: "orders".to_string(),
                key: WorldKey::from_u64(103),
                tuple: make_order_tuple("bob", 200, "shipped"),
            },
        ],
    );
    session.apply_batch(batch1).expect("apply seed");

    let alice_key = encode_secondary_key(b"alice");
    let bob_key = encode_secondary_key(b"bob");

    let mut alice_orders = session
        .query_secondary_index("orders", "customer_id", &alice_key)
        .expect("query alice");
    alice_orders.sort();
    assert_eq!(
        alice_orders,
        vec![WorldKey::from_u64(101), WorldKey::from_u64(102)],
        "both orders for customer alice must be returned"
    );

    let bob_orders = session
        .query_secondary_index("orders", "customer_id", &bob_key)
        .expect("query bob");
    assert_eq!(bob_orders, vec![WorldKey::from_u64(103)]);

    // 2. Changing an unrelated field preserves the index root
    let rev1_snap = session.pin_revision(1).expect("pin rev 1");
    let rev1_index_root = rev1_snap.revision.secondary_index_roots["orders:customer_id"];

    let batch2 = WorldBatch::new(
        1,
        "update-unrelated-field",
        vec![WorldBatchOp::Upsert {
            relation: "orders".to_string(),
            key: WorldKey::from_u64(101),
            tuple: make_order_tuple("alice", 999, "processing"), // customer 'alice' unchanged
        }],
    );
    let rc2 = session.apply_batch(batch2).expect("apply unrelated update");
    let rev2_snap = session.pin_revision(2).expect("pin rev 2");
    let rev2_index_root = rev2_snap.revision.secondary_index_roots["orders:customer_id"];

    assert_eq!(
        rev1_index_root, rev2_index_root,
        "changing an unrelated field (total/status) must preserve the secondary index root byte-for-byte"
    );
    // Path cost for unrelated update must allocate only primary trie path (<= 6 nodes)
    assert!(
        rc2.objects_written <= 6,
        "unrelated field update wrote {} objects; expected <= 6 primary path nodes",
        rc2.objects_written
    );

    // 3. Changing the customer moves exactly one membership
    let batch3 = WorldBatch::new(
        2,
        "move-customer",
        vec![WorldBatchOp::Upsert {
            relation: "orders".to_string(),
            key: WorldKey::from_u64(101),
            tuple: make_order_tuple("bob", 999, "processing"), // alice -> bob
        }],
    );
    session.apply_batch(batch3).expect("apply customer move");

    let alice_orders_after_move = session
        .query_secondary_index("orders", "customer_id", &alice_key)
        .expect("query alice");
    assert_eq!(
        alice_orders_after_move,
        vec![WorldKey::from_u64(102)],
        "alice must now only have order 102"
    );

    let mut bob_orders_after_move = session
        .query_secondary_index("orders", "customer_id", &bob_key)
        .expect("query bob");
    bob_orders_after_move.sort();
    assert_eq!(
        bob_orders_after_move,
        vec![WorldKey::from_u64(101), WorldKey::from_u64(103)],
        "bob must now have order 101 moved from alice plus order 103"
    );

    // 4. Deletion removes exactly that membership
    let batch4 = WorldBatch::new(
        3,
        "delete-alice-order",
        vec![WorldBatchOp::Remove {
            relation: "orders".to_string(),
            key: WorldKey::from_u64(102),
        }],
    );
    session.apply_batch(batch4).expect("apply delete");

    let alice_orders_deleted = session
        .query_secondary_index("orders", "customer_id", &alice_key)
        .expect("query alice after delete");
    assert!(
        alice_orders_deleted.is_empty(),
        "alice has 0 orders remaining and must return empty list"
    );

    let mut bob_orders_survived = session
        .query_secondary_index("orders", "customer_id", &bob_key)
        .expect("query bob after alice delete");
    bob_orders_survived.sort();
    assert_eq!(
        bob_orders_survived,
        vec![WorldKey::from_u64(101), WorldKey::from_u64(103)],
        "bob's orders must be undisturbed by alice's order deletion"
    );

    // 5. Reopen preserves these results
    session.close().expect("close session");
    let cold_session = WorldSession::open(&dir).expect("reopen session");

    let cold_alice = cold_session
        .query_secondary_index("orders", "customer_id", &alice_key)
        .expect("cold query alice");
    assert!(cold_alice.is_empty());

    let mut cold_bob = cold_session
        .query_secondary_index("orders", "customer_id", &bob_key)
        .expect("cold query bob");
    cold_bob.sort();
    assert_eq!(
        cold_bob,
        vec![WorldKey::from_u64(101), WorldKey::from_u64(103)]
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_large_uneven_customer_groups_do_not_copy_entire_group() {
    let dir = test_dir("whale_no_copy");
    let manifest = indexed_manifest();
    let mut session = WorldSession::create(&dir, manifest).expect("create world");

    // Ingest 500 orders for customer 'whale'
    let mut seed_ops = Vec::with_capacity(500);
    for i in 0..500u64 {
        seed_ops.push(WorldBatchOp::Upsert {
            relation: "orders".to_string(),
            key: WorldKey::from_u64(i),
            tuple: make_order_tuple("whale", i * 10, "open"),
        });
    }

    session
        .apply_batch(WorldBatch::new(0, "seed-whale", seed_ops))
        .expect("seed whale");

    let count_before = session.node_store.node_count();

    // Add 1 more order for the whale customer
    let edit_batch = WorldBatch::new(
        1,
        "add-one-to-whale",
        vec![WorldBatchOp::Upsert {
            relation: "orders".to_string(),
            key: WorldKey::from_u64(9999),
            tuple: make_order_tuple("whale", 12345, "open"),
        }],
    );
    let receipt = session
        .apply_batch(edit_batch)
        .expect("apply edit to whale");
    let count_after = session.node_store.node_count();
    let new_objects = count_after - count_before;

    // For a group of 500 keys, the inner persistent trie traverses depth <= 4.
    // Outer trie traverses depth <= 4. Primary trie traverses depth <= 4.
    // Total path nodes <= 5 (prim) + 4 (sec_inner) + 4 (sec_outer) = 13.
    // It must NEVER copy the 500 orders.
    assert!(
        new_objects <= 13,
        "adding to a 500-order group wrote {new_objects} objects; expected <= 13 path nodes, not full group copy"
    );
    assert_eq!(receipt.changed_keys_count, 1);

    // Verify all 501 orders are returned
    let whale_key = encode_secondary_key(b"whale");
    let whale_orders = session
        .query_secondary_index("orders", "customer_id", &whale_key)
        .expect("query whale");
    assert_eq!(whale_orders.len(), 501);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_indexed_workload_costs_zero_one_and_several_indexes() {
    let dir = test_dir("workload_costs");
    let manifest = indexed_manifest();
    let mut session = WorldSession::create(&dir, manifest).expect("create world");

    // Seed 1,000 items in inventory (0 indexes), 1,000 in orders (1 index), 1,000 in shipments (2 indexes)
    let mut seed_ops = Vec::new();
    for i in 0..1000u64 {
        seed_ops.push(WorldBatchOp::Upsert {
            relation: "inventory".to_string(),
            key: WorldKey::from_u64(i),
            tuple: WorldTuple::from_str(&format!("sku_{i}:qty_100")),
        });
        seed_ops.push(WorldBatchOp::Upsert {
            relation: "orders".to_string(),
            key: WorldKey::from_u64(i),
            tuple: make_order_tuple(&format!("cust_{:03}", i % 50), i * 10, "open"),
        });
        seed_ops.push(WorldBatchOp::Upsert {
            relation: "shipments".to_string(),
            key: WorldKey::from_u64(i),
            tuple: make_shipment_tuple(
                &format!("cust_{:03}", i % 50),
                &format!("carrier_{}", i % 5),
                "in_transit",
            ),
        });
    }
    session
        .apply_batch(WorldBatch::new(0, "large-seed", seed_ops))
        .expect("seed");

    // Case 1: Zero indexes (inventory) - 1 key update
    let b1 = session.node_store.node_count();
    let rc1 = session
        .apply_batch(WorldBatch::new(
            1,
            "inv-update",
            vec![WorldBatchOp::Upsert {
                relation: "inventory".to_string(),
                key: WorldKey::from_u64(500),
                tuple: WorldTuple::from_str("sku_500:qty_999_updated"),
            }],
        ))
        .expect("inv update");
    let n1 = session.node_store.node_count() - b1;
    // Primary-only bound: D_prim <= 5
    assert!(
        n1 <= 6,
        "0-index update wrote {n1} objects; expected <= 6 path nodes (rc objects={})",
        rc1.objects_written
    );

    // Case 2: One index (orders) - Non-indexed field change (total/status)
    let b2 = session.node_store.node_count();
    let rc2 = session
        .apply_batch(WorldBatch::new(
            2,
            "order-non-idx-edit",
            vec![WorldBatchOp::Upsert {
                relation: "orders".to_string(),
                key: WorldKey::from_u64(500),
                tuple: make_order_tuple("cust_000", 99999, "completed"), // customer unchanged (500 % 50 == 0)
            }],
        ))
        .expect("order non-idx update");
    let n2 = session.node_store.node_count() - b2;
    // Secondary index is skipped; only primary path is written: <= 6
    assert!(
        n2 <= 6,
        "1-index non-indexed field change wrote {n2} objects; expected <= 6 nodes (rc objects={})",
        rc2.objects_written
    );

    // Case 3: One index (orders) - Indexed-field change (customer change)
    let b3 = session.node_store.node_count();
    let rc3 = session
        .apply_batch(WorldBatch::new(
            3,
            "order-idx-change",
            vec![WorldBatchOp::Upsert {
                relation: "orders".to_string(),
                key: WorldKey::from_u64(500),
                tuple: make_order_tuple("cust_049", 99999, "completed"), // cust_000 -> cust_049
            }],
        ))
        .expect("order idx change");
    let n3 = session.node_store.node_count() - b3;
    // D_prim (<=5) + 2 * (D_sec_inner (<=4) + D_sec_outer (<=4)) <= 5 + 16 = 21
    assert!(
        n3 <= 21,
        "1-index indexed field change wrote {n3} objects; expected <= 21 path nodes (rc objects={})",
        rc3.objects_written
    );

    // Case 4: One index (orders) - Delete 1 record
    let b4 = session.node_store.node_count();
    let rc4 = session
        .apply_batch(WorldBatch::new(
            4,
            "order-delete",
            vec![WorldBatchOp::Remove {
                relation: "orders".to_string(),
                key: WorldKey::from_u64(500),
            }],
        ))
        .expect("order delete");
    let n4 = session.node_store.node_count() - b4;
    // D_prim (<=5) + D_sec_inner (<=4) + D_sec_outer (<=4) <= 13
    assert!(
        n4 <= 13,
        "1-index delete wrote {n4} objects; expected <= 13 path nodes (rc objects={})",
        rc4.objects_written
    );

    // Case 5: Several indexes (shipments with 2 indexes: customer_id, carrier) - Insert
    let b5 = session.node_store.node_count();
    let rc5 = session
        .apply_batch(WorldBatch::new(
            5,
            "shipment-insert-2-idx",
            vec![WorldBatchOp::Upsert {
                relation: "shipments".to_string(),
                key: WorldKey::from_u64(9999),
                tuple: make_shipment_tuple("cust_001", "carrier_fast", "booked"),
            }],
        ))
        .expect("shipment insert");
    let n5 = session.node_store.node_count() - b5;
    // D_prim (<=5) + 2 * (D_sec_inner (<=4) + D_sec_outer (<=4)) <= 5 + 16 = 21
    assert!(
        n5 <= 21,
        "2-index insert wrote {n5} objects; expected <= 21 path nodes (rc objects={})",
        rc5.objects_written
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_invalid_secondary_index_declaration_rejected() {
    let dir = test_dir("invalid_sec_decl");
    let bad_manifest = WorldManifest::new(
        "bad-manifest",
        "2026-10-04T00:00:00Z",
        Digest::of(brix_canon::Domain::Value, b"bad:v1"),
        vec![RelationDecl::new(
            "orders",
            vec!["order_id".to_string()],
            vec!["customer_id".to_string(), "total".to_string()],
            vec!["non_existent_field".to_string()], // NOT in value_fields or key_fields!
        )],
    );

    let result = WorldSession::create(&dir, bad_manifest);
    match result {
        Err(WorldError::InvalidIndexDeclaration(_)) => {}
        Err(e) => panic!("expected InvalidIndexDeclaration, got error: {e}"),
        Ok(_) => panic!("expected InvalidIndexDeclaration, but session creation succeeded"),
    }
}
