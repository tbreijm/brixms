//! Lane E — P8 scale de-risking: release-mode throughput and latency measurement
//! harness for the persistent world runtime (ADR-0046; see
//! `docs/planning/persistent-world-runtime-plan.md` §1, P7, P8, §5 matrix).
//!
//! This file adds NO new assertions about correctness (that is covered by
//! `p5_adversarial_probe.rs` and friends) and makes NO changes to `src/`. It only
//! *measures* the existing public `WorldSession` + `WorldNetwork` path, plus the
//! lower-level `soc_core::store` primitives those types are built on (both are
//! public dependencies of `brix-kb`), to find out where ingestion and edit-latency
//! time goes before attempting P8 (1M resident keyed facts, >=100 linked modules).
//!
//! Every test here is `#[ignore]` by default. Run with:
//!   cargo test -p brix-kb --release --test world_throughput_probe -- --ignored --nocapture --test-threads=1
//!
//! All numbers printed are measured in-process with `std::time::Instant` and the
//! `FileNodeStore::io_stats()` counters that already exist in `soc-core` (reads,
//! bytes_read, writes, bytes_written, files_synced, directories_synced). Nothing
//! is fabricated or silently extrapolated: any number not actually measured by a
//! run of this file is clearly absent from its output.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use brix_canon::{Digest, Domain};
use brix_kb::world::{
    RelationDecl, TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldManifest, WorldNetwork,
    WorldSession, WorldTuple,
};
use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};
use soc_core::store::{FileNodeStore, StoreIoStats, TrieMap};

fn test_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "brix_p8_throughput_{}_{}_{}",
        name,
        std::process::id(),
        // Distinguish successive calls within the same process (several
        // probes create more than one directory).
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create test dir");
    dir
}

/// Helper to compile a WorldNetwork from module sources (mirrors
/// `p5_adversarial_probe::make_network`).
fn make_network(sources: &[(&str, &str)]) -> WorldNetwork {
    let mut map = BTreeMap::new();
    for (name, src) in sources {
        map.insert(name.to_string(), src.to_string());
    }
    let loader = |name: &str| map.get(name).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .expect("module graph load failed");
    let linked = graph.link().expect("module graph link failed");
    WorldNetwork::from_program(&linked).expect("relational lowering and network creation failed")
}

fn upsert_op(rel: &str, key_num: u64, fields: &[(&str, &str)]) -> WorldBatchOp {
    let mut rec = TupleRecord::new();
    for (k, v) in fields {
        rec.set_str(*k, v);
    }
    WorldBatchOp::Upsert {
        relation: rel.to_string(),
        key: WorldKey::from_u64(key_num),
        tuple: rec.to_tuple(),
    }
}

/// Same 3-relation linked model as P5 (`orders` / `inventory` / `shipping` joined
/// into `fulfillment`, decided by `dispatch`), so these numbers are directly
/// comparable to `docs/performance/world-runtime-p5-results.md`.
const LINKED_MODEL_SRC: &str = r#"
rel input orders: { id: Str, customer: Str, sku: Str, qty: Int, express: Str } key id
rel input inventory: { id: Str, sku: Str, available: Int } key id
rel input shipping: { id: Str, sku: Str, carrier: Str, lead_days: Int } key id

rel derived fulfillment =
    select { order_id: o.id, customer: o.customer, sku: o.sku, qty: o.qty, express: o.express, available: i.available, carrier: s.carrier, lead_days: s.lead_days }
    from o in orders, i in inventory, s in shipping
    where o.sku == i.sku and o.sku == s.sku

decide dispatch for f in fulfillment {
    propose ship_express priority 10 when f.express == "yes" and f.available >= f.qty = "air_express"
    propose ship_standard priority 20 when f.express != "yes" and f.available >= f.qty = "ground_standard"
    propose backorder priority 50 when f.available < f.qty = "backorder_hold"
}
"#;

fn make_linked_manifest() -> WorldManifest {
    WorldManifest::new(
        "p8_throughput_world",
        "2026-10-04T00:00:00Z",
        Digest::of(brix_canon::Domain::Value, b"p8:throughput:linked:model:v1"),
        vec![
            RelationDecl::new(
                "root::orders",
                vec!["id".to_string()],
                vec![
                    "customer".to_string(),
                    "sku".to_string(),
                    "qty".to_string(),
                    "express".to_string(),
                ],
                vec![],
            ),
            RelationDecl::new(
                "root::inventory",
                vec!["id".to_string()],
                vec!["sku".to_string(), "available".to_string()],
                vec!["sku".to_string()],
            ),
            RelationDecl::new(
                "root::shipping",
                vec!["id".to_string()],
                vec![
                    "sku".to_string(),
                    "carrier".to_string(),
                    "lead_days".to_string(),
                ],
                vec![],
            ),
        ],
    )
}

/// Generates a linked workload of `num_orders` orders spread over `num_skus`
/// SKUs (with matching inventory and shipping rows), same shape as the P5
/// probe's 10k fixture but parametrized by size.
fn seed_ops(num_skus: u64, num_orders: u64) -> Vec<WorldBatchOp> {
    let mut ops = Vec::with_capacity((num_skus * 2 + num_orders) as usize);
    for i in 0..num_skus {
        let sku = format!("SKU-{i}");
        ops.push(upsert_op(
            "root::inventory",
            100_000_000 + i,
            &[
                ("id", &format!("INV-{i}")),
                ("sku", &sku),
                ("available", "25"),
            ],
        ));
    }
    for i in 0..num_skus {
        let sku = format!("SKU-{i}");
        let carrier = if i % 2 == 0 {
            "AirExpress"
        } else {
            "GroundStandard"
        };
        ops.push(upsert_op(
            "root::shipping",
            200_000_000 + i,
            &[
                ("id", &format!("SHIP-{i}")),
                ("sku", &sku),
                ("carrier", carrier),
                ("lead_days", "2"),
            ],
        ));
    }
    for i in 0..num_orders {
        let sku = format!("SKU-{}", i % num_skus);
        let qty = format!("{}", (i % 10) + 1);
        let express = if i % 3 == 0 { "yes" } else { "no" };
        ops.push(upsert_op(
            "root::orders",
            i + 1,
            &[
                ("id", &format!("ORD-{}", i + 1)),
                ("customer", &format!("CUST-{}", i % 500)),
                ("sku", &sku),
                ("qty", &qty),
                ("express", express),
            ],
        ));
    }
    ops
}

/// (num_skus, num_orders, total_rows) for a given target row count, keeping the
/// same ~80% orders / 10% inventory / 10% shipping ratio as the P5 10k fixture
/// (1,000 SKUs / 8,000 orders).
fn sizing_for(total_rows: u64) -> (u64, u64, u64) {
    let num_skus = (total_rows / 10).max(1);
    let num_orders = total_rows - 2 * num_skus;
    (num_skus, num_orders, num_skus * 2 + num_orders)
}

fn io_delta(before: StoreIoStats, after: StoreIoStats) -> StoreIoStats {
    StoreIoStats {
        reads: after.reads - before.reads,
        bytes_read: after.bytes_read - before.bytes_read,
        writes: after.writes - before.writes,
        bytes_written: after.bytes_written - before.bytes_written,
        physical_writes: after.physical_writes - before.physical_writes,
        files_synced: after.files_synced - before.files_synced,
        directories_synced: after.directories_synced - before.directories_synced,
    }
}

fn print_io(label: &str, s: &StoreIoStats) {
    println!(
        "  {label}: reads={} bytes_read={} writes={} bytes_written={} physical_writes={} files_synced={} directories_synced={}",
        s.reads, s.bytes_read, s.writes, s.bytes_written, s.physical_writes, s.files_synced, s.directories_synced
    );
}

// ============================================================================
// A. Ingestion throughput at scale: single big batch, like P5's p01 but
//    parametrized. Measures wall time, rows/s, objects written, bytes written,
//    intermediate deltas, and the FileNodeStore io breakdown (reads, writes,
//    fsyncs).
// ============================================================================
fn run_ingest_probe(total_rows: u64, label: &str) {
    let (num_skus, num_orders, actual_rows) = sizing_for(total_rows);
    let dir = test_dir(&format!("ingest_{label}"));
    let manifest = make_linked_manifest();

    let mut session = WorldSession::create(&dir, manifest).expect("create world session");
    let mut network = make_network(&[("root", LINKED_MODEL_SRC)]);

    let ops = seed_ops(num_skus, num_orders);
    assert_eq!(ops.len() as u64, actual_rows, "exact row-count contract");

    let io_before = session.node_store.io_stats();
    let batch = WorldBatch::new(0, format!("{label}-seed"), ops);

    let t_session = Instant::now();
    let receipt = session
        .apply_batch(batch.clone())
        .expect("apply batch to session");
    let session_elapsed = t_session.elapsed();

    let t_network = Instant::now();
    let net_report = network.apply_batch(&batch).expect("apply batch to network");
    let network_elapsed = t_network.elapsed();

    let io_after = session.node_store.io_stats();
    let io = io_delta(io_before, io_after);

    let rows_per_sec = actual_rows as f64 / session_elapsed.as_secs_f64();

    println!("PROBE ingest[{label}]: rows={actual_rows} (skus={num_skus}, orders={num_orders})");
    println!(
        "  session.apply_batch: {:.3?} ({:.1} rows/s), objects_written={}, changed_keys={}",
        session_elapsed, rows_per_sec, receipt.objects_written, receipt.changed_keys_count
    );
    println!(
        "  network.apply_batch: {:.3?}, intermediate_deltas={}, settlements={}",
        network_elapsed,
        net_report.intermediate_deltas_count,
        net_report
            .settlements
            .values()
            .map(|m| m.len())
            .sum::<usize>()
    );
    print_io("node_store io delta", &io);
    println!(
        "  ns/row (session)={:.0}  bytes_written/row={:.1}",
        session_elapsed.as_nanos() as f64 / actual_rows as f64,
        io.bytes_written as f64 / actual_rows as f64
    );
}

#[test]
#[ignore]
fn probe_ingest_1k() {
    run_ingest_probe(1_000, "1k");
}

#[test]
#[ignore]
fn probe_ingest_10k() {
    run_ingest_probe(10_000, "10k");
}

#[test]
#[ignore]
fn probe_ingest_100k() {
    run_ingest_probe(100_000, "100k");
}

/// 1,000,000-row ingestion. NOT run automatically as part of routine probing —
/// per the lane brief, only attempt this if the 100k numbers extrapolate to
/// well under the ~15 minute budget. Left here (ignored) so a future lane can
/// invoke it directly once the fsync-path fix (see the companion doc) lands.
#[test]
#[ignore]
fn probe_ingest_1m() {
    run_ingest_probe(1_000_000, "1m");
}

// ============================================================================
// B. Batch granularity: is the fixed per-`apply_batch` cost (revision file
//    write+fsync, HEAD rename+fsync, directory fsyncs) a large or small
//    fraction of total ingestion time? Fixed total row count, varying batch
//    size. Uses a single un-joined relation (no network) to isolate the
//    WorldSession store/fsync path from join/decision overhead.
// ============================================================================
fn make_single_relation_manifest() -> WorldManifest {
    WorldManifest::new(
        "p8_batch_granularity_world",
        "2026-10-04T00:00:00Z",
        Digest::of(brix_canon::Domain::Value, b"p8:throughput:single:model:v1"),
        vec![RelationDecl::new(
            "root::items",
            vec!["id".to_string()],
            vec!["val".to_string()],
            vec![],
        )],
    )
}

fn single_op(key_num: u64) -> WorldBatchOp {
    upsert_op(
        "root::items",
        key_num,
        &[
            ("id", &format!("ITEM-{key_num}")),
            ("val", &format!("value-{key_num}")),
        ],
    )
}

fn run_batch_granularity_probe(total_rows: u64, batch_size: u64, label: &str) {
    let dir = test_dir(&format!("batchsz_{label}"));
    let manifest = make_single_relation_manifest();
    let mut session = WorldSession::create(&dir, manifest).expect("create world session");

    let io_before = session.node_store.io_stats();
    let t_start = Instant::now();

    let mut key = 0u64;
    let mut base_rev = 0u64;
    let mut batch_count = 0u64;
    while key < total_rows {
        let this_batch_len = batch_size.min(total_rows - key);
        let ops: Vec<WorldBatchOp> = (0..this_batch_len).map(|i| single_op(key + i)).collect();
        let batch = WorldBatch::new(base_rev, format!("{label}-b{batch_count}"), ops);
        session
            .apply_batch(batch)
            .expect("apply batch in granularity probe");
        base_rev += 1;
        key += this_batch_len;
        batch_count += 1;
    }
    let elapsed = t_start.elapsed();
    let io_after = session.node_store.io_stats();
    let io = io_delta(io_before, io_after);

    println!(
        "PROBE batch-granularity[{label}]: total_rows={total_rows} batch_size={batch_size} batches={batch_count}"
    );
    println!(
        "  wall={:.3?} ({:.1} rows/s), ns/row={:.0}, ns/batch={:.0}",
        elapsed,
        total_rows as f64 / elapsed.as_secs_f64(),
        elapsed.as_nanos() as f64 / total_rows as f64,
        elapsed.as_nanos() as f64 / batch_count as f64
    );
    print_io("node_store io delta", &io);
    println!(
        "  fsync_calls_per_batch={:.2}",
        (io.files_synced + io.directories_synced) as f64 / batch_count as f64
    );
}

const GRANULARITY_TOTAL_ROWS: u64 = 2_000;

#[test]
#[ignore]
fn probe_batch_granularity_size_1() {
    run_batch_granularity_probe(GRANULARITY_TOTAL_ROWS, 1, "sz1");
}

#[test]
#[ignore]
fn probe_batch_granularity_size_100() {
    run_batch_granularity_probe(GRANULARITY_TOTAL_ROWS, 100, "sz100");
}

#[test]
#[ignore]
fn probe_batch_granularity_size_all() {
    run_batch_granularity_probe(GRANULARITY_TOTAL_ROWS, GRANULARITY_TOTAL_ROWS, "szall");
}

// ============================================================================
// C. One-fact edit latency, broken into network propagation (pure in-memory,
//    `WorldNetwork::apply_batch` makes no filesystem calls — verified by
//    inspection of network.rs) vs store write + fsync + publication
//    (`WorldSession::apply_batch`).
// ============================================================================
fn run_edit_latency_probe(total_rows: u64, label: &str) {
    let (num_skus, num_orders, actual_rows) = sizing_for(total_rows);
    let dir = test_dir(&format!("edit_{label}"));
    let manifest = make_linked_manifest();

    let mut session = WorldSession::create(&dir, manifest).expect("create world session");
    let mut network = make_network(&[("root", LINKED_MODEL_SRC)]);

    let seed = seed_ops(num_skus, num_orders);
    let seed_batch = WorldBatch::new(0, format!("{label}-seed"), seed);
    session
        .apply_batch(seed_batch.clone())
        .expect("seed session");
    network.apply_batch(&seed_batch).expect("seed network");

    // Targeted single-fact edit: flip order #1's express flag.
    let edit_op = upsert_op(
        "root::orders",
        1,
        &[
            ("id", "ORD-1"),
            ("customer", "CUST-0"),
            ("sku", "SKU-0"),
            ("qty", "5"),
            ("express", "no"),
        ],
    );
    let edit_batch = WorldBatch::new(1, format!("{label}-edit"), vec![edit_op]);

    let io_before = session.node_store.io_stats();

    let t_session = Instant::now();
    let edit_receipt = session
        .apply_batch(edit_batch.clone())
        .expect("apply edit to session");
    let session_elapsed = t_session.elapsed();

    let t_network = Instant::now();
    let net_report = network
        .apply_batch(&edit_batch)
        .expect("apply edit to network");
    let network_elapsed = t_network.elapsed();

    let io_after = session.node_store.io_stats();
    let io = io_delta(io_before, io_after);

    println!("PROBE edit-latency[{label}]: world_rows={actual_rows}");
    println!(
        "  session.apply_batch (store write + fsync + publication): {:.3?}, objects_written={}",
        session_elapsed, edit_receipt.objects_written
    );
    println!(
        "  network.apply_batch (pure in-memory propagation, no I/O): {:.3?}, intermediate_deltas={}, settlements={}",
        network_elapsed,
        net_report.intermediate_deltas_count,
        net_report.settlements.values().map(|m| m.len()).sum::<usize>()
    );
    print_io("node_store io delta", &io);
    println!(
        "  total edit latency (session+network, sequential)={:.3?}",
        session_elapsed + network_elapsed
    );
}

#[test]
#[ignore]
fn probe_edit_latency_1k() {
    run_edit_latency_probe(1_000, "1k");
}

#[test]
#[ignore]
fn probe_edit_latency_10k() {
    run_edit_latency_probe(10_000, "10k");
}

#[test]
#[ignore]
fn probe_edit_latency_100k() {
    run_edit_latency_probe(100_000, "100k");
}

// ============================================================================
// D. Store-phase breakdown: decompose a single ingestion batch's cost into
//    (A) pure in-memory trie construction (hashing + node allocation, zero I/O
//    since freshly-built nodes are never `Node::Lazy` so `resolve_node` never
//    touches the store), (B) `persist_to_store` (node file creation + rename,
//    NO fsync — `FileNodeStore::put_node` only queues the path), and
//    (C) `flush()` (the actual `fsync`/`sync_all` calls). This isolates the
//    fsync-per-object-file hypothesis directly against the production
//    `TrieMap` + `FileNodeStore` types (no new storage code, no src/ edits).
// ============================================================================
fn run_store_phase_breakdown(n: u64, label: &str) {
    let dir = test_dir(&format!("phase_{label}"));
    let store_root = dir.join("store_bench");
    fs::create_dir_all(&store_root).expect("create store bench dir");
    let mut store = FileNodeStore::new(&store_root).expect("create file node store");

    // Phase A: pure in-memory trie construction.
    let t_a = Instant::now();
    let mut trie: TrieMap<WorldKey, WorldTuple> = TrieMap::new();
    for i in 0..n {
        let key = WorldKey::from_u64(i);
        let val = WorldTuple::from_str(&format!("value-{i}-payload-padding-0123456789"));
        let (next, _stats) = trie
            .insert_with_store(key, val, &store)
            .expect("insert_with_store (in-memory, no Lazy nodes, no store I/O expected)");
        trie = next;
    }
    let phase_a = t_a.elapsed();
    let io_after_a = store.io_stats();

    // Phase B: persist_to_store — node file creation + atomic rename, no fsync.
    let t_b = Instant::now();
    let objects_written = trie.persist_to_store(&mut store);
    let phase_b = t_b.elapsed();
    let io_after_b = store.io_stats();

    // Phase C: flush() — the fsync/sync_all calls.
    let t_c = Instant::now();
    store.flush().expect("flush store");
    let phase_c = t_c.elapsed();
    let io_after_c = store.io_stats();

    // Hashing-cost proxy: independent loop computing content digests of
    // similarly-sized payloads, NOT the same calls made internally during
    // Phase A/B (those aren't separately instrumentable without touching
    // src/), but representative of the per-node hashing cost paid during
    // both trie key hashing and node content-addressing.
    let t_hash = Instant::now();
    let mut acc = 0u8;
    for i in 0..n {
        let payload = format!("value-{i}-payload-padding-0123456789");
        let d = Digest::of(Domain::Value, payload.as_bytes());
        acc ^= d.as_bytes()[0];
    }
    let hash_proxy_elapsed = t_hash.elapsed();
    std::hint::black_box(acc);

    println!("PROBE store-phase-breakdown[{label}]: n={n}");
    println!(
        "  phase A (in-memory trie build):     {:.3?} ({:.0} ns/op)",
        phase_a,
        phase_a.as_nanos() as f64 / n as f64
    );
    print_io("    io after phase A (expect all-zero)", &io_after_a);
    println!(
        "  phase B (persist_to_store, no fsync): {:.3?} ({:.0} ns/node), objects_written={}",
        phase_b,
        phase_b.as_nanos() as f64 / objects_written.max(1) as f64,
        objects_written
    );
    print_io(
        "    io delta after phase B",
        &io_delta(io_after_a, io_after_b),
    );
    println!(
        "  phase C (flush/fsync):              {:.3?} ({:.0} ns/fsync)",
        phase_c,
        phase_c.as_nanos() as f64
            / (io_after_c.files_synced + io_after_c.directories_synced
                - io_after_b.files_synced
                - io_after_b.directories_synced)
                .max(1) as f64
    );
    print_io(
        "    io delta after phase C",
        &io_delta(io_after_b, io_after_c),
    );
    println!(
        "  hashing-cost proxy (independent, {} digests): {:.3?} ({:.0} ns/digest)",
        n,
        hash_proxy_elapsed,
        hash_proxy_elapsed.as_nanos() as f64 / n as f64
    );
    println!(
        "  phase totals: A+B+C={:.3?} (compare to end-to-end session.apply_batch wall time for the same n)",
        phase_a + phase_b + phase_c
    );
}

#[test]
#[ignore]
fn probe_store_phase_breakdown_1k() {
    run_store_phase_breakdown(1_000, "1k");
}

#[test]
#[ignore]
fn probe_store_phase_breakdown_10k() {
    run_store_phase_breakdown(10_000, "10k");
}

#[test]
#[ignore]
fn probe_store_phase_breakdown_100k() {
    run_store_phase_breakdown(100_000, "100k");
}

// ============================================================================
// E. Growth check: does per-row ingestion cost (ns/row) or edit latency grow
//    with world size? Runs the three ingestion sizes back-to-back in one
//    process and prints a table, so growth is visible without cross-run
//    variance. This is the same work as probes A/C above but sequenced for
//    direct side-by-side comparison; run it on its own for the table in the
//    report.
// ============================================================================
#[test]
#[ignore]
fn probe_growth_table_ingest() {
    for (n, label) in [(1_000u64, "1k"), (10_000, "10k"), (100_000, "100k")] {
        run_ingest_probe(n, label);
    }
}

#[test]
#[ignore]
fn probe_growth_table_edit() {
    for (n, label) in [(1_000u64, "1k"), (10_000, "10k"), (100_000, "100k")] {
        run_edit_latency_probe(n, label);
    }
}

/// Smoke test (NOT ignored): checks the harness itself compiles and the smallest
/// probe runs correctly in debug, without asserting any performance bound (this
/// file measures; it does not gate). Keeps the file from silently bit-rotting
/// between release-mode probing sessions.
#[test]
fn harness_smoke_test() {
    let (num_skus, num_orders, actual_rows) = sizing_for(50);
    assert_eq!(actual_rows, 50);
    let dir = test_dir("smoke");
    let manifest = make_linked_manifest();
    let mut session = WorldSession::create(&dir, manifest).expect("create world session");
    let ops = seed_ops(num_skus, num_orders);
    let batch = WorldBatch::new(0, "smoke-seed", ops);
    let receipt = session.apply_batch(batch).expect("apply smoke batch");
    assert_eq!(receipt.changed_keys_count as u64, actual_rows);

    // Store-phase breakdown harness itself, at trivial scale.
    run_store_phase_breakdown(20, "smoke");

    // Batch granularity harness itself, at trivial scale.
    run_batch_granularity_probe(20, 5, "smoke");
}
