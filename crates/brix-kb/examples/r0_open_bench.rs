//! R0 cold-open baseline harness (benchmark only; no runtime behaviour is changed).
//!
//! Builds a synthetic persistent world with N facts across three input relations
//! (`orders::rows`, `stock::rows`, `routes::rows`), a join, a filter, a projection, a grouped
//! count and a `decide` block, loads it in ten equal batches, then measures:
//!
//! * wall time of every load batch (superlinear growth shows up as rising per-batch time);
//! * `WorldSession::open` (cold open) in a **fresh child process**, so RSS is not polluted by
//!   the loader's high-water mark;
//! * `all_settlements()` (world decisions) right after the cold open;
//! * one single-row correction batch right after the cold open;
//! * RSS (via `ps -o rss=`) after open and after the one-row batch, and on-disk bytes.
//!
//! The result is printed as one JSON object on stdout.
//!
//! Usage (from the crate or workspace root):
//!
//! ```text
//! cargo run --release -p brix-kb --example r0_open_bench -- 10000
//! cargo run --release -p brix-kb --example r0_open_bench -- 100000
//! cargo run --release -p brix-kb --example r0_open_bench -- 1000000
//! ```
//!
//! The first argument is N (total facts, default 10,000). Batches are N/10 rows each.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use brix_kb::world::{TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldSession};

/// Set by the parent when re-invoking this binary as the cold-open child.
const CHILD_ENV: &str = "R0_OPEN_BENCH_CHILD_DIR";
const BATCHES: usize = 10;

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
    select { id: o.id, sku: o.sku, qty: o.qty, available: s.available, carrier: r.carrier }
    from o in orders::rows, s in stock::rows, r in routes::rows
    where o.sku == s.sku and o.sku == r.sku and o.qty > 0
rel derived sku_order_counts =
    select { sku: f.sku, order_count: count() }
    from f in fulfillment
    group by f.sku
decide dispatch for f in fulfillment per id {
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

/// Row `i` of the synthetic N-fact world. The first `skus` rows are stock, the next `skus`
/// are routes, the rest are orders. Every order joins exactly one stock and one route row.
fn row(i: usize, n: usize) -> WorldBatchOp {
    let skus = n / 10;
    if i < skus {
        upsert(
            "stock::rows",
            &format!("S-{i}"),
            &[("sku", &format!("SKU-{i}")), ("available", "25")],
        )
    } else if i < skus * 2 {
        let j = i - skus;
        upsert(
            "routes::rows",
            &format!("R-{j}"),
            &[("sku", &format!("SKU-{j}")), ("carrier", "ground")],
        )
    } else {
        let j = i - skus * 2;
        upsert(
            "orders::rows",
            &format!("O-{j}"),
            &[("sku", &format!("SKU-{}", j % skus)), ("qty", "5")],
        )
    }
}

fn rss_kb(pid: u32) -> Option<u64> {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

fn dir_bytes(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|e| {
            let p = e.path();
            if p.is_dir() {
                dir_bytes(&p)
            } else {
                e.metadata().map(|m| m.len()).unwrap_or(0)
            }
        })
        .sum()
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Child mode: cold open in a fresh process, then decisions and one-row batch.
fn run_child(dir: &Path) {
    let pid = std::process::id();
    let rss_before = rss_kb(pid);

    let t = Instant::now();
    let mut session = WorldSession::open(dir).expect("cold open");
    let open_ms = ms(t.elapsed());
    let rss_after_open = rss_kb(pid);

    let t = Instant::now();
    let settlements = session.all_settlements();
    let decisions_ms = ms(t.elapsed());
    let decision_count: usize = settlements.values().map(|m| m.len()).sum();

    let batch = WorldBatch::new(
        session.current_revision(),
        "r0-one-row",
        vec![upsert(
            "orders::rows",
            "O-0",
            &[("sku", "SKU-0"), ("qty", "30")],
        )],
    );
    let t = Instant::now();
    let receipt = session
        .apply_batch(batch)
        .expect("one-row batch after cold open");
    let one_row_ms = ms(t.elapsed());
    let rss_after_one_row = rss_kb(pid);

    let out = serde_json::json!({
        "rss_before_open_kb": rss_before,
        "cold_open_ms": open_ms,
        "rss_after_open_kb": rss_after_open,
        "decisions_ms": decisions_ms,
        "decision_count": decision_count,
        "one_row_batch_ms": one_row_ms,
        "one_row_changed_keys": receipt.changed_keys_count,
        "one_row_objects_written": receipt.objects_written,
        "rss_after_one_row_kb": rss_after_one_row,
    });
    println!("CHILD_JSON {out}");
}

/// Parent mode: build the world, load it in batches, then run the child cold-open probe.
fn run_parent(n: usize) {
    assert!(
        n >= 10 && n.is_multiple_of(BATCHES * 10),
        "N must be a multiple of 100"
    );
    let batch_rows = n / BATCHES;
    let dir: PathBuf =
        std::env::temp_dir().join(format!("r0_open_bench_{n}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);

    let t = Instant::now();
    let mut session = WorldSession::from_program_with_sources(&dir, "root", &sources())
        .expect("initialize world with source closure");
    let init_ms = ms(t.elapsed());

    let mut batch_stats = Vec::with_capacity(BATCHES);
    let load_start = Instant::now();
    for b in 0..BATCHES {
        let ops: Vec<WorldBatchOp> = (b * batch_rows..(b + 1) * batch_rows)
            .map(|i| row(i, n))
            .collect();
        let t = Instant::now();
        session
            .apply_batch(WorldBatch::new(
                session.current_revision(),
                format!("load-{b}"),
                ops,
            ))
            .expect("load batch");
        batch_stats.push(serde_json::json!({
            "batch": b,
            "rows": batch_rows,
            "ms": ms(t.elapsed()),
        }));
    }
    let load_ms = ms(load_start.elapsed());
    let rss_after_load = rss_kb(std::process::id());
    let facts: usize = session.relations.values().map(|r| r.len()).sum();
    session.close().expect("flush world before cold open");
    let disk_bytes = dir_bytes(&dir);

    let exe = std::env::current_exe().expect("current exe");
    let child = Command::new(exe)
        .env(CHILD_ENV, &dir)
        .output()
        .expect("spawn cold-open child");
    assert!(
        child.status.success(),
        "cold-open child failed:\n{}",
        String::from_utf8_lossy(&child.stderr)
    );
    let stdout = String::from_utf8_lossy(&child.stdout);
    let child_json: serde_json::Value = stdout
        .lines()
        .find_map(|l| l.strip_prefix("CHILD_JSON "))
        .map(|j| serde_json::from_str(j).expect("child json"))
        .expect("child emitted CHILD_JSON");

    let report = serde_json::json!({
        "bench": "r0_open_bench",
        "facts": n,
        "facts_resident": facts,
        "batches": BATCHES,
        "batch_rows": batch_rows,
        "init_ms": init_ms,
        "load_total_ms": load_ms,
        "load_batches": batch_stats,
        "rss_after_load_kb": rss_after_load,
        "disk_bytes": disk_bytes,
        "cold_open": child_json,
    });
    println!("{report}");
    let _ = fs::remove_dir_all(&dir);
}

fn main() {
    if let Some(dir) = std::env::var_os(CHILD_ENV) {
        run_child(Path::new(&dir));
        return;
    }
    let n: usize = std::env::args()
        .nth(1)
        .map(|s| s.parse().expect("N must be an integer"))
        .unwrap_or(10_000);
    run_parent(n);
}
