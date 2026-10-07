//! Skew and memory control qualification test suite (P7).
//!
//! Validates:
//! 1. Fair, bounded, resumable operator scheduling preserves identical semantics
//!    and decision roots across quanta 1, 16, and 256, verified against reference evaluation.
//! 2. 99.9% vs 0.1% submodel skew isolation: an update to a 0.1% submodel touches zero
//!    operators in the 99.9% submodel and completes with bounded $O(1)$ work.
//! 3. Resumable high-fanout join expansion yields per quantum and produces accurate diagnostics
//!    identifying hot join keys and expensive operators.
//! 4. Execution budget enforcement stops oversized work early and leaves the committed revision untouched.
//! 5. Bounded node store LRU cache eviction and I/O counter tracking.
//! 6. Retention pins protect reader revisions and audit checkpoints during history compaction,
//!    with measurable byte reclamation upon unpinning.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use brix_canon::{Digest, Domain};
use brix_kb::world::network::{NetworkLimits, WorldNetwork};
use brix_kb::world::reference;
use brix_kb::world::{
    RelationDecl, TupleRecord, WorldBatch, WorldBatchOp, WorldError, WorldKey, WorldManifest,
    WorldSession, WorldTuple,
};
use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};
use soc_core::store::{FileNodeStore, NodeStore};

fn temp_test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brix_skew_test_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create test dir");
    dir
}

fn upsert_op(rel: &str, id: &str, fields: &[(&str, &str)]) -> WorldBatchOp {
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

// ---------------------------------------------------------------------------
// 1. Quantum Invariance Test (Quanta = 1, 16, 256)
// ---------------------------------------------------------------------------
#[test]
fn test_scheduling_quantum_invariance_and_reference_parity() {
    let src = r#"
rel input users: { id: Str, dept: Str } key id
rel input orders: { id: Str, user_id: Str, amount: Str } key id

rel derived user_orders =
    select { user_id: u.id, dept: u.dept }
    from u in users, o in orders
    where u.id == o.user_id and o.amount == "100"

decide dept_winner for uo in user_orders per user_id {
    propose p1 priority 1 when uo.dept == "eng" = uo.user_id
    propose p2 priority 2 when uo.dept == "sales" = uo.user_id
}
"#;

    let dir = temp_test_dir("quantum_invariance");
    let manifest = WorldManifest::new(
        "quantum-test",
        "2026-10-06T00:00:00Z",
        Digest::of(Domain::Value, b"initial"),
        vec![
            RelationDecl::new(
                "root::users",
                vec!["id".into()],
                vec!["dept".into()],
                vec![],
            ),
            RelationDecl::new(
                "root::orders",
                vec!["id".into()],
                vec!["user_id".into(), "amount".into()],
                vec![],
            ),
        ],
    );

    let _session = WorldSession::create(&dir, manifest).unwrap();
    let sources: BTreeMap<String, String> = BTreeMap::from([("root".into(), src.into())]);
    let loader = |name: &str| sources.get(name).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).unwrap();
    let linked = graph.link().unwrap();

    // Prepare batch operations
    let ops = vec![
        upsert_op("root::users", "u1", &[("dept", "eng")]),
        upsert_op("root::users", "u2", &[("dept", "sales")]),
        upsert_op("root::users", "u3", &[("dept", "eng")]),
        upsert_op(
            "root::orders",
            "o1",
            &[("user_id", "u1"), ("amount", "100")],
        ),
        upsert_op(
            "root::orders",
            "o2",
            &[("user_id", "u1"), ("amount", "100")],
        ), // duplicate support for distinct
        upsert_op(
            "root::orders",
            "o3",
            &[("user_id", "u2"), ("amount", "100")],
        ),
        upsert_op("root::orders", "o4", &[("user_id", "u3"), ("amount", "50")]), // filtered out
    ];

    // Evaluate on net with quantum 1, 16, 256
    let mut net_q1 = WorldNetwork::from_program(&linked)
        .unwrap()
        .with_scheduling_quantum(1);
    let mut net_q16 = WorldNetwork::from_program(&linked)
        .unwrap()
        .with_scheduling_quantum(16);
    let mut net_q256 = WorldNetwork::from_program(&linked)
        .unwrap()
        .with_scheduling_quantum(256);

    let rep_q1 = net_q1.apply_ops(&ops).unwrap();
    let rep_q16 = net_q16.apply_ops(&ops).unwrap();
    let rep_q256 = net_q256.apply_ops(&ops).unwrap();

    assert_eq!(net_q1.decision_root(), net_q16.decision_root());
    assert_eq!(net_q16.decision_root(), net_q256.decision_root());
    assert_eq!(rep_q1.settlements, rep_q16.settlements);
    assert_eq!(rep_q16.settlements, rep_q256.settlements);

    // Apply cascading retractions and conflicting proposals
    let retraction_ops = vec![
        WorldBatchOp::Remove {
            relation: "root::orders".into(),
            key: WorldKey::from_str("o1"),
        },
        upsert_op("root::users", "u2", &[("dept", "eng")]), // update u2 dept
    ];

    let rep2_q1 = net_q1.apply_ops(&retraction_ops).unwrap();
    let rep2_q16 = net_q16.apply_ops(&retraction_ops).unwrap();
    let rep2_q256 = net_q256.apply_ops(&retraction_ops).unwrap();

    assert_eq!(net_q1.decision_root(), net_q16.decision_root());
    assert_eq!(net_q16.decision_root(), net_q256.decision_root());
    assert_eq!(rep2_q1.settlements, rep2_q16.settlements);
    assert_eq!(rep2_q16.settlements, rep2_q256.settlements);

    // Verify against independent reference evaluator
    let mut base_relations: BTreeMap<String, BTreeMap<WorldKey, WorldTuple>> = BTreeMap::new();
    for (rel_name, trie) in &net_q256.base_relations {
        let mut map = BTreeMap::new();
        for (k, v) in trie {
            map.insert(k.clone(), v.clone());
        }
        base_relations.insert(rel_name.clone(), map);
    }

    let ref_prog = reference::from_program(&linked).unwrap();
    let ref_state = reference::evaluate(&ref_prog, &base_relations).unwrap();
    for (decide, entities) in net_q256.all_settlements() {
        for (entity, dec) in entities {
            let ref_dec = ref_state
                .settlements
                .get(&decide)
                .and_then(|m| m.get(&entity))
                .expect("reference settlement present");
            assert_eq!(dec.entity_id, ref_dec.entity_id);
            assert_eq!(dec.candidate_name, ref_dec.candidate_name);
            assert_eq!(dec.priority, ref_dec.priority);
            assert_eq!(dec.phase, ref_dec.phase);
            assert_eq!(dec.value.to_string(), ref_dec.value.to_string());
            assert_eq!(dec.calendar_key, ref_dec.calendar_key);
        }
    }
}

// ---------------------------------------------------------------------------
// 2. Submodel Skew Isolation (99.9% vs 0.1%)
// ---------------------------------------------------------------------------
#[test]
fn test_submodel_skew_isolation_99_9_vs_0_1() {
    let src = r#"
rel input sub_a: { id: Str, tag: Str } key id
rel input sub_b: { id: Str, tag: Str } key id

rel derived a_active =
    select { id: a.id, tag: a.tag }
    from a in sub_a
    where a.tag == "active"

rel derived b_active =
    select { id: b.id, tag: b.tag }
    from b in sub_b
    where b.tag == "active"

decide decide_b for b in b_active per id {
    propose win priority 1 when b.tag == "active" = b.id
}
"#;

    let dir = temp_test_dir("skew_isolation");
    let manifest = WorldManifest::new(
        "skew-iso",
        "2026-10-06T00:00:00Z",
        Digest::of(Domain::Value, b"initial"),
        vec![
            RelationDecl::new("root::sub_a", vec!["id".into()], vec!["tag".into()], vec![]),
            RelationDecl::new("root::sub_b", vec!["id".into()], vec!["tag".into()], vec![]),
        ],
    );

    let mut session = WorldSession::create(&dir, manifest).unwrap();
    let sources: BTreeMap<String, String> = BTreeMap::from([("root".into(), src.into())]);
    session.save_program_closure("root", &sources).unwrap();

    // Ingest 999 facts into submodel A (99.9%) and 1 fact into submodel B (0.1%)
    let mut initial_ops = Vec::new();
    for i in 0..999 {
        initial_ops.push(upsert_op(
            "root::sub_a",
            &format!("a_{i}"),
            &[("tag", "active")],
        ));
    }
    initial_ops.push(upsert_op("root::sub_b", "b_0", &[("tag", "active")]));

    let batch1 = WorldBatch::new(0, "batch-1", initial_ops);
    session.apply_batch(batch1).unwrap();

    // Now update submodel B with 1 fact (e.g. tag changed to "inactive")
    let update_b_ops = vec![upsert_op("root::sub_b", "b_0", &[("tag", "inactive")])];

    let mut net = session.network.clone().unwrap();
    let report = net.apply_ops(&update_b_ops).unwrap();

    // Skew isolation guarantees:
    // 1. sub_a and a_active received ZERO deltas
    assert_eq!(
        report.diagnostics.relation_deltas.get("root::sub_a"),
        None,
        "Submodel A received zero deltas during submodel B mutation"
    );
    assert_eq!(
        report.diagnostics.relation_deltas.get("root::a_active"),
        None,
        "Derived relation in Submodel A received zero deltas"
    );

    // 2. Only sub_b and b_active had deltas
    assert!(report
        .diagnostics
        .relation_deltas
        .contains_key("root::sub_b"));

    // 3. Total operator work for this batch is strictly bounded and small
    let total_work: u64 = report.diagnostics.operator_work.values().sum();
    assert!(
        total_work <= 10,
        "Mutation on 0.1% submodel required minimal work (actual: {total_work}) despite 99.9% sibling"
    );
}

// ---------------------------------------------------------------------------
// 3. Resumable High-Fanout Join Expansion & Diagnostics
// ---------------------------------------------------------------------------
#[test]
fn test_resumable_join_expansion_and_fanout_diagnostics() {
    let src = r#"
rel input customers: { id: Str, status: Str } key id
rel input orders: { id: Str, cust_id: Str, amount: Str } key id

rel derived high_fanout =
    select { cust: c.id, ord: o.id }
    from c in customers, o in orders
    where c.id == o.cust_id
"#;

    let dir = temp_test_dir("fanout_diag");
    let manifest = WorldManifest::new(
        "fanout-test",
        "2026-10-06T00:00:00Z",
        Digest::of(Domain::Value, b"initial"),
        vec![
            RelationDecl::new(
                "root::customers",
                vec!["id".into()],
                vec!["status".into()],
                vec![],
            ),
            RelationDecl::new(
                "root::orders",
                vec!["id".into()],
                vec!["cust_id".into(), "amount".into()],
                vec![],
            ),
        ],
    );

    let _session = WorldSession::create(&dir, manifest).unwrap();
    let sources: BTreeMap<String, String> = BTreeMap::from([("root".into(), src.into())]);
    let loader = |name: &str| sources.get(name).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).unwrap();
    let linked = graph.link().unwrap();

    let mut net = WorldNetwork::from_program(&linked)
        .unwrap()
        .with_scheduling_quantum(16);

    // Populate 200 orders pointing to "c1"
    let mut order_ops = Vec::new();
    for i in 0..200 {
        order_ops.push(upsert_op(
            "root::orders",
            &format!("ord_{i}"),
            &[("cust_id", "c1"), ("amount", "50")],
        ));
    }
    net.apply_ops(&order_ops).unwrap();

    // Now insert customer "c1" triggering 200 join matches across quantum slices
    let cust_op = vec![upsert_op("root::customers", "c1", &[("status", "vip")])];
    let rep = net.apply_ops(&cust_op).unwrap();

    // Verify diagnostics
    assert_eq!(rep.derived_tuples_inserted, 200);
    let expensive_op = rep.diagnostics.expensive_operator();
    assert!(expensive_op.is_some(), "Expensive operator identified");

    let hot_key = rep.diagnostics.hot_join_key();
    assert!(hot_key.is_some(), "Hot join key identified");
    let (key_str, count) = hot_key.unwrap();
    assert!(key_str.contains("c1"));
    assert_eq!(count, 200);
}

// ---------------------------------------------------------------------------
// 4. Execution Resource Budgeting & Rollback Atomicity
// ---------------------------------------------------------------------------
#[test]
fn test_execution_budget_exhaustion_leaves_committed_revision_unchanged() {
    let src = r#"
rel input items: { id: Str, num: Str } key id
rel derived filtered =
    select { id: i.id, num: i.num }
    from i in items
    where i.num == "10"
"#;

    let dir = temp_test_dir("budget_atomicity");
    let manifest = WorldManifest::new(
        "budget-test",
        "2026-10-06T00:00:00Z",
        Digest::of(Domain::Value, b"initial"),
        vec![RelationDecl::new(
            "root::items",
            vec!["id".into()],
            vec!["num".into()],
            vec![],
        )],
    );

    let mut session = WorldSession::create(&dir, manifest).unwrap();
    let sources: BTreeMap<String, String> = BTreeMap::from([("root".into(), src.into())]);
    session.save_program_closure("root", &sources).unwrap();

    // Ingest initial item at rev 1
    session
        .apply_batch(WorldBatch::new(
            0,
            "b1",
            vec![upsert_op("root::items", "item_0", &[("num", "10")])],
        ))
        .unwrap();

    assert_eq!(session.current_revision(), 1);

    // Propose batch with 50 items under constrained budget max_work: 5
    let mut large_ops = Vec::new();
    for i in 1..=50 {
        large_ops.push(upsert_op(
            "root::items",
            &format!("item_{i}"),
            &[("num", "10")],
        ));
    }

    let limits = NetworkLimits {
        max_work: Some(5),
        max_matches: None,
        max_expressions: None,
        max_queued_deltas: None,
    };

    let result =
        session.apply_batch_bounded(WorldBatch::new(1, "b2-oversized", large_ops), Some(&limits));

    assert!(
        matches!(result, Err(WorldError::BudgetExhausted)),
        "Expected BudgetExhausted error"
    );

    // Verify session remains at revision 1 and item_1 was not committed
    assert_eq!(session.current_revision(), 1);
    assert!(session
        .get("root::items", &WorldKey::from_str("item_1"))
        .unwrap()
        .is_none());
    assert!(session
        .get("root::items", &WorldKey::from_str("item_0"))
        .unwrap()
        .is_some());
}

// ---------------------------------------------------------------------------
// 5. Node Store LRU Cache Eviction & I/O Tracking
// ---------------------------------------------------------------------------
#[test]
fn test_node_store_lru_cache_eviction_and_counters() {
    let dir = temp_test_dir("node_cache_lru");
    let mut store = FileNodeStore::new(&dir).unwrap();
    store.set_cache_capacity(5);

    // Insert 20 nodes into the store
    let mut digests = Vec::new();
    for i in 0..20 {
        let data = format!("node-content-{}", i);
        let bytes = data.into_bytes();
        let digest = Digest::of(Domain::Value, &bytes);
        store.put_node(digest, bytes);
        digests.push(digest);
    }
    store.flush().unwrap();

    // Cache should hold at most 5 nodes
    assert!(
        store.cached_nodes_count() <= 5,
        "Cache size capped at capacity 5"
    );

    // Read back all 20 nodes to exercise cache hits, misses, and evictions
    for d in &digests {
        let node = store.get_node(d);
        assert!(node.is_some());
    }

    let stats_after_reads = store.io_stats();
    assert!(stats_after_reads.reads >= 20);
    assert!(stats_after_reads.cache_misses > 0);
    assert!(
        stats_after_reads.evictions > 0,
        "Nodes were evicted during read traversal to respect capacity"
    );
    assert!(store.cached_nodes_count() <= 5);
}

// ---------------------------------------------------------------------------
// 6. Retention Pinning & History Compaction
// ---------------------------------------------------------------------------
#[test]
fn test_retention_pinning_protects_revisions_and_reclaims_bytes() {
    let dir = temp_test_dir("retention_compaction");
    let manifest = WorldManifest::new(
        "retention-test",
        "2026-10-06T00:00:00Z",
        Digest::of(Domain::Value, b"initial"),
        vec![RelationDecl::new(
            "root::logs",
            vec!["id".into()],
            vec!["msg".into()],
            vec![],
        )],
    );

    let mut session = WorldSession::create(&dir, manifest).unwrap();

    // Commit 5 revisions
    for rev in 1..=5 {
        session
            .apply_batch(WorldBatch::new(
                rev - 1,
                format!("b_{rev}"),
                vec![upsert_op(
                    "root::logs",
                    &format!("log_{rev}"),
                    &[("msg", "hello")],
                )],
            ))
            .unwrap();
    }

    assert_eq!(session.current_revision(), 5);

    let bytes_before = session.measure_retained_bytes().unwrap();
    assert!(bytes_before > 0);

    // Pin revision 2 with an active reader pin
    let pin_id = session.pin_reader(2).unwrap();

    // Pin revision 4 as a checkpoint pin
    session.pin_checkpoint(4).unwrap();

    // Compact history up to revision 5
    let report = session.compact_history(5).unwrap();

    // Revisions 1 and 3 should be reclaimed.
    // Revisions 2, 4, 5 (and 0) should be retained.
    assert_eq!(report.revisions_reclaimed, 2);
    assert!(report.pinned_revisions.contains(&2));
    assert!(report.pinned_revisions.contains(&4));
    assert!(report.pinned_revisions.contains(&5));

    // Verify revision 1 is no longer openable
    assert!(matches!(
        session.pin_revision(1),
        Err(WorldError::RevisionNotFound(1))
    ));
    assert!(matches!(
        session.pin_revision(3),
        Err(WorldError::RevisionNotFound(3))
    ));

    // Verify pinned revisions 2 and 4 are still openable!
    assert!(session.pin_revision(2).is_ok());
    assert!(session.pin_revision(4).is_ok());

    let bytes_after_first_compact = session.measure_retained_bytes().unwrap();
    assert!(bytes_after_first_compact < bytes_before);

    // Now unpin reader pin on revision 2
    assert!(session.unpin_reader(pin_id));

    // Compact history again
    let report2 = session.compact_history(5).unwrap();
    assert_eq!(report2.revisions_reclaimed, 1); // revision 2 is now reclaimed!

    // Verify revision 2 is now gone
    assert!(matches!(
        session.pin_revision(2),
        Err(WorldError::RevisionNotFound(2))
    ));

    // Checkpoint at revision 4 is still preserved
    assert!(session.pin_revision(4).is_ok());

    let bytes_final = session.measure_retained_bytes().unwrap();
    assert!(bytes_final < bytes_after_first_compact);
}
