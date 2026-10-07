//! Executable-world qualification: persistence and maintained decisions run in
//! the SAME session/transaction. Unlike Lane E's phase-isolation probe, these
//! timings include source loading, network updates and durable publication.
//! Run individual ignored tests in release mode with --nocapture.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use brix_kb::world::{TupleRecord, Value, WorldBatch, WorldBatchOp, WorldKey, WorldSession};

fn sources() -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "orders".into(),
            "export rel input rows: { id: Str, sku: Str, qty: Int } key id".into(),
        ),
        (
            "stock".into(),
            "export rel input rows: { id: Str, sku: Str, available: Int } key id".into(),
        ),
        (
            "routes".into(),
            "export rel input rows: { id: Str, sku: Str, carrier: Str } key id".into(),
        ),
        (
            "root".into(),
            r#"
use orders
use stock
use routes
rel derived fulfillment =
    select { id: o.id, qty: o.qty, available: s.available, carrier: r.carrier }
    from o in orders::rows, s in stock::rows, r in routes::rows
    where o.sku == s.sku and o.sku == r.sku
decide dispatch for f in fulfillment {
    propose ship priority 10 when f.qty <= f.available = f.carrier
    propose hold priority 20 when f.qty > f.available = "backorder"
}
"#
            .into(),
        ),
    ])
}

fn upsert(relation: &str, id: &str, fields: &[(&str, &str)]) -> WorldBatchOp {
    let mut tuple = TupleRecord::new();
    tuple.set_str("id", id);
    for (name, value) in fields {
        tuple.set_str(*name, value);
    }
    WorldBatchOp::Upsert {
        relation: relation.into(),
        key: WorldKey::from_str(id),
        tuple: tuple.to_tuple(),
    }
}

fn assert_decision(session: &WorldSession, id: &str, expected: Option<&str>) {
    let network = session
        .network
        .as_ref()
        .expect("executable session must have network");
    let decision = network.get_settlement("root::dispatch", id);
    assert_eq!(
        decision.map(|d| d.value),
        expected.map(|s| Value::Str(s.into()))
    );
}

fn temp_dir(rows: usize) -> PathBuf {
    std::env::temp_dir().join(format!(
        "brix_integrated_scale_{rows}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn run(rows: usize) {
    assert_eq!(rows % 10, 0);
    let skus = rows / 10;
    let orders = rows - 2 * skus;
    let root = temp_dir(rows);
    println!("INTEGRATED artifact_dir={}", root.display());
    let start = Instant::now();
    let mut session = WorldSession::from_program_with_sources(&root, "root", &sources())
        .expect("initialize executable world with pinned source closure");
    let init = start.elapsed();
    assert!(session.network.is_some());
    let before = session.node_store.io_stats();
    let start = Instant::now();
    let mut canonical_input_bytes = 0;
    let mut batches = 0;
    let mut ops = Vec::new();
    // Bounded staging avoids a second million-row request clone. Batch size is
    // recorded in output; this is an ingestion policy, not a world size limit.
    const BATCH_ROWS: usize = 10_000;
    for i in 0..rows {
        let op = if i < skus {
            let sku = format!("SKU-{i}");
            upsert(
                "stock::rows",
                &format!("S-{i}"),
                &[("sku", &sku), ("available", "25")],
            )
        } else if i < skus * 2 {
            let j = i - skus;
            let sku = format!("SKU-{j}");
            upsert(
                "routes::rows",
                &format!("R-{j}"),
                &[("sku", &sku), ("carrier", "ground")],
            )
        } else {
            let j = i - skus * 2;
            let sku = format!("SKU-{}", j % skus);
            upsert(
                "orders::rows",
                &format!("O-{j}"),
                &[("sku", &sku), ("qty", "5")],
            )
        };
        if let WorldBatchOp::Upsert { key, tuple, .. } = &op {
            canonical_input_bytes += key.as_bytes().len() + tuple.as_bytes().len();
        }
        ops.push(op);
        if ops.len() == BATCH_ROWS || i + 1 == rows {
            session
                .apply_batch(WorldBatch::new(
                    session.current_revision(),
                    format!("seed-{batches}"),
                    std::mem::take(&mut ops),
                ))
                .expect("atomic executable ingestion batch");
            batches += 1;
            if batches % 10 == 0 {
                println!(
                    "INTEGRATED progress rows={} elapsed_ms={}",
                    i + 1,
                    start.elapsed().as_millis()
                );
            }
        }
    }
    let ingest = start.elapsed();
    let after = session.node_store.io_stats();
    assert_decision(&session, "O-0", Some("ground"));
    assert_decision(&session, &format!("O-{}", orders - 1), Some("ground"));
    let total: usize = session.relations.values().map(|r| r.len()).sum();
    assert_eq!(total, rows, "actual resident keyed facts");
    let old = session
        .pin_revision(session.current_revision())
        .expect("pin source snapshot");
    let old_value = old.get("orders::rows", &WorldKey::from_str("O-0")).unwrap();
    let old_decision_root = session.network.as_ref().unwrap().decision_root();

    let edit_before = session.node_store.io_stats();
    let edit_start = Instant::now();
    let receipt = session
        .apply_batch(WorldBatch::new(
            session.current_revision(),
            "correct-order",
            vec![upsert(
                "orders::rows",
                "O-0",
                &[("sku", "SKU-0"), ("qty", "30")],
            )],
        ))
        .expect("one-fact correction including decision and persistence");
    let edit = edit_start.elapsed();
    let edit_after = session.node_store.io_stats();
    assert_decision(&session, "O-0", Some("backorder"));
    assert_decision(&session, "O-1", Some("ground"));
    assert_ne!(
        session.network.as_ref().unwrap().decision_root(),
        old_decision_root
    );
    assert_eq!(
        old.get("orders::rows", &WorldKey::from_str("O-0")).unwrap(),
        old_value
    );
    assert_ne!(
        session
            .get("orders::rows", &WorldKey::from_str("O-0"))
            .unwrap(),
        old_value
    );

    let start = Instant::now();
    session
        .apply_batch(WorldBatch::new(
            session.current_revision(),
            "retract-order",
            vec![WorldBatchOp::Remove {
                relation: "orders::rows".into(),
                key: WorldKey::from_str("O-1"),
            }],
        ))
        .expect("retraction removes settlement atomically");
    let retract = start.elapsed();
    assert_decision(&session, "O-1", None);
    let revision = session.current_revision();
    let root_digest = session.network.as_ref().unwrap().decision_root().to_hex();
    drop(old);
    drop(session);

    let reopen_start = Instant::now();
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "integrated_reopen_child",
            "--ignored",
            "--nocapture",
        ])
        .env("BRIX_INTEGRATED_REOPEN", &root)
        .env("BRIX_INTEGRATED_REVISION", revision.to_string())
        .env("BRIX_INTEGRATED_DECISION_ROOT", &root_digest)
        .output()
        .expect("start independent reopen process");
    let reopen = reopen_start.elapsed();
    assert!(
        child.status.success(),
        "fresh-process reopen failed:\n{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    println!("{}", String::from_utf8_lossy(&child.stdout));
    println!(
        "INTEGRATED {}",
        serde_json::json!({
            "rows": rows, "modules": 4, "batch_rows": BATCH_ROWS, "batches": batches,
            "canonical_input_bytes": canonical_input_bytes, "init_ms": init.as_millis(),
            "ingest_ms": ingest.as_millis(), "rows_per_second": rows as f64 / ingest.as_secs_f64(),
            "ingest_nodes": after.writes-before.writes, "ingest_bytes": after.bytes_written-before.bytes_written,
            "ingest_file_syncs": after.files_synced-before.files_synced,
            "ingest_directory_syncs": after.directories_synced-before.directories_synced,
            "edit_ms": edit.as_micros() as f64 / 1000.0, "edit_nodes": receipt.objects_written,
            "edit_bytes": edit_after.bytes_written-edit_before.bytes_written,
            "edit_file_syncs": edit_after.files_synced-edit_before.files_synced,
            "edit_directory_syncs": edit_after.directories_synced-edit_before.directories_synced,
            "retract_ms": retract.as_micros() as f64 / 1000.0,
            "fresh_process_reopen_ms": reopen.as_millis(), "revision": revision,
            "decision_root": root_digest,
            "scope": "four-module integrated runtime; not the full hundred-module P8 matrix"
        })
    );
    std::fs::remove_dir_all(root).expect("clean successful probe artifacts");
}

#[test]
#[ignore = "helper invoked by integrated scale probes with a world path"]
fn integrated_reopen_child() {
    let Some(root) = std::env::var_os("BRIX_INTEGRATED_REOPEN") else {
        return;
    };
    let start = Instant::now();
    let session = WorldSession::open(Path::new(&root)).expect("reopen executable source closure");
    assert_eq!(
        session.current_revision().to_string(),
        std::env::var("BRIX_INTEGRATED_REVISION").unwrap()
    );
    assert_decision(&session, "O-0", Some("backorder"));
    assert_decision(&session, "O-1", None);
    assert_decision(&session, "O-2", Some("ground"));
    assert_eq!(
        session.network.as_ref().unwrap().decision_root().to_hex(),
        std::env::var("BRIX_INTEGRATED_DECISION_ROOT").unwrap()
    );
    println!(
        "INTEGRATED reopen_ms={} decision_root_verified=true",
        start.elapsed().as_millis()
    );
}

#[test]
#[ignore = "release-build integrated durability qualification"]
fn integrated_1k() {
    run(1_000);
}
#[test]
#[ignore = "release-build integrated durability qualification"]
fn integrated_10k() {
    run(10_000);
}
#[test]
#[ignore = "release-build integrated durability qualification"]
fn integrated_100k() {
    run(100_000);
}
#[test]
#[ignore = "release-build integrated durability qualification, resource intensive"]
fn integrated_1m() {
    run(1_000_000);
}
