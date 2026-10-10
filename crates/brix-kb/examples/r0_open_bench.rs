//! R0 cold-open baseline harness (benchmark only; no runtime behaviour is changed).
//!
//! Builds a synthetic persistent world with N facts across three input relations
//! (`orders::rows`, `stock::rows`, `routes::rows`), a join, a filter, a projection, a grouped
//! count and a `decide` block, loads it in ten equal batches, then measures:
//!
//! * wall time of every load batch (superlinear growth shows up as rising per-batch time);
//! * `WorldSession::open` (cold open) in a **fresh child process**, so RSS is not polluted by
//!   the loader's high-water mark;
//! * `all_settlements().unwrap()` (world decisions) right after the cold open;
//! * one single-row correction batch right after the cold open;
//! * RSS (via `ps -o rss=`) after open and after the one-row batch, and on-disk bytes;
//! * the fallback path: the HEAD record is rewritten as a pre-M4 `@2` record, then two fresh
//!   processes open it (the first replays and refreshes the state-cache sidecar, the second
//!   restores from it).
//!
//! Every open reports `open_ms`, `open_mode`, `replay_reason` and `state_source`.
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

use brix_kb::world::{
    TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldPaths, WorldRevision, WorldSession,
    REVISION_SCHEMA_V2,
};

/// Set by the parent when re-invoking this binary as the cold-open child.
const CHILD_ENV: &str = "R0_OPEN_BENCH_CHILD_DIR";
/// Set to make the child only open the world and report how.
const OPEN_ONLY_ENV: &str = "R0_OPEN_BENCH_OPEN_ONLY";
/// Set to keep the generated world directory (the cold-open child can then be re-run on it
/// with `R0_OPEN_BENCH_CHILD_DIR=<dir>` for profiling).
const KEEP_ENV: &str = "R0_OPEN_BENCH_KEEP";
/// Set to stop the child after the decisions read (no one-row batch, so the world is not mutated).
const NO_BATCH_ENV: &str = "R0_OPEN_BENCH_NO_BATCH";
const BATCHES_DEFAULT: usize = 10;
/// Number of load batches (default 10); smaller batches approximate chunked persistence.
const BATCHES_ENV: &str = "R0_OPEN_BENCH_BATCHES";
/// Set to stop after the load (no cold-open child, no fallback probes).
/// Set (with the child dir) to open by full replay and exit, for peak-RSS measurements.
const REPLAY_ONLY_ENV: &str = "R0_OPEN_BENCH_REPLAY_ONLY";
const LOAD_ONLY_ENV: &str = "R0_OPEN_BENCH_LOAD_ONLY";

fn batches() -> usize {
    std::env::var(BATCHES_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|b| *b > 0)
        .unwrap_or(BATCHES_DEFAULT)
}

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

/// Cumulative CPU time of `pid` as `ps -o cputime=` prints it (`[h:]mm:ss.cc`), in seconds.
fn cpu_time(pid: u32) -> Option<f64> {
    let out = Command::new("ps")
        .args(["-o", "cputime=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut secs = 0.0;
    for part in text.trim().split(':') {
        secs = secs * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(secs)
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

fn open_report(session: &WorldSession, wall_ms: f64) -> serde_json::Value {
    serde_json::json!({
        "open_ms_wall": wall_ms,
        "open_ms": session.open_ms,
        "open_mode": session.open_mode.as_str(),
        "replay_reason": session.replay_reason,
        "state_source": session.state_source,
        "state_refreshed": session.state_refreshed,
    })
}

/// Child mode: cold open in a fresh process, then decisions and one-row batch.
fn run_child(dir: &Path) {
    let pid = std::process::id();
    let rss_before = rss_kb(pid);

    // Opening the node store alone (pack index load) is the floor of any open.
    let t = Instant::now();
    let store = soc_core::store::FileNodeStore::new(dir).expect("node store");
    let store_new_ms = ms(t.elapsed());
    drop(store);

    let t = Instant::now();
    let mut session = if std::env::var_os(REPLAY_ONLY_ENV).is_some() {
        WorldSession::open_replay(dir).expect("replay open")
    } else {
        WorldSession::open(dir).expect("cold open")
    };
    let open_ms = ms(t.elapsed());
    let rss_after_open = rss_kb(pid);
    let mut open = open_report(&session, open_ms);
    open["file_store_new_ms"] = serde_json::json!(store_new_ms);
    if std::env::var_os(OPEN_ONLY_ENV).is_some() || std::env::var_os(REPLAY_ONLY_ENV).is_some() {
        println!(
            "CHILD_JSON {}",
            serde_json::json!({"open": open, "rss_after_open_kb": rss_after_open})
        );
        return;
    }

    let t = Instant::now();
    let settlements = session.all_settlements().unwrap();
    let decisions_ms = ms(t.elapsed());
    let decision_count: usize = settlements.values().map(|m| m.len()).sum();

    if std::env::var_os(NO_BATCH_ENV).is_some() {
        println!(
            "CHILD_JSON {}",
            serde_json::json!({
                "open": open,
                "decisions_ms": decisions_ms,
                "decision_count": decision_count,
                "rss_after_decisions_kb": rss_kb(pid),
            })
        );
        return;
    }
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
        "open": open,
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
        n >= 10 && n.is_multiple_of(batches() * 10),
        "N must be a multiple of 10 x the batch count"
    );
    let nb = batches();
    let batch_rows = n / nb;
    let dir: PathBuf =
        std::env::temp_dir().join(format!("r0_open_bench_{n}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);

    let t = Instant::now();
    let mut session = WorldSession::from_program_with_sources(&dir, "root", &sources())
        .expect("initialize world with source closure");
    let init_ms = ms(t.elapsed());

    let mut batch_stats = Vec::with_capacity(nb);
    let load_start = Instant::now();
    for b in 0..nb {
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
        let profile = session.last_commit_profile.as_ref().map(|p| {
            serde_json::json!({
                "phases_ms": p.phases.iter().map(|(n, v)| (n.to_string(), serde_json::json!(v))).collect::<serde_json::Map<_, _>>(),
                "state_node_reads": p.state_node_reads,
                "state_bytes_read": p.state_bytes_read,
                "state_bytes_written": p.state_bytes_written,
            })
        });
        batch_stats.push(serde_json::json!({
            "batch": b,
            "rows": batch_rows,
            "ms": ms(t.elapsed()),
            "rss_kb": rss_kb(std::process::id()),
            "cpu": cpu_time(std::process::id()),
            "profile": profile,
        }));
    }
    let load_ms = ms(load_start.elapsed());
    let rss_after_load = rss_kb(std::process::id());
    let facts: usize = session.relations.values().map(|r| r.len()).sum();
    session.close().expect("flush world before cold open");
    let disk_bytes = dir_bytes(&dir);
    if std::env::var(LOAD_ONLY_ENV).is_ok() {
        println!(
            "{}",
            serde_json::json!({
                "bench": "r0_open_bench", "facts": n, "batches": nb, "load_total_ms": load_ms,
                "rss_after_load_kb": rss_after_load, "disk_bytes": disk_bytes,
                "load_batches": batch_stats,
            })
        );
        if std::env::var_os(KEEP_ENV).is_none() {
            let _ = fs::remove_dir_all(&dir);
        } else {
            eprintln!("KEPT {}", dir.display());
        }
        return;
    }

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

    // Fallback: a pre-M4 world. First open replays and refreshes the sidecar, second restores.
    let fallback = {
        let paths = WorldPaths::new(&dir);
        let seq: u64 = fs::read_to_string(paths.head())
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let v = serde_json::from_slice(&fs::read(paths.revision_file(seq)).unwrap()).unwrap();
        let mut rev = WorldRevision::from_json(&v).unwrap();
        rev.schema = REVISION_SCHEMA_V2.to_string();
        rev.network_state_root = None;
        rev.revision_digest = rev.compute_digest_for_record();
        fs::write(
            paths.revision_file(seq),
            serde_json::to_vec_pretty(&rev.to_json()).unwrap(),
        )
        .unwrap();
        fs::write(
            paths.head(),
            format!("{} {}\n", seq, rev.revision_digest.to_hex()),
        )
        .unwrap();
        let open_only = || {
            let out = Command::new(std::env::current_exe().unwrap())
                .env(CHILD_ENV, &dir)
                .env(OPEN_ONLY_ENV, "1")
                .output()
                .expect("spawn open-only child");
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let j = stdout
                .lines()
                .find_map(|l| l.strip_prefix("CHILD_JSON "))
                .expect("child json")
                .to_string();
            serde_json::from_str::<serde_json::Value>(&j).unwrap()
        };
        let first = open_only();
        let second = open_only();
        serde_json::json!({"first_open": first, "second_open": second})
    };

    let report = serde_json::json!({
        "bench": "r0_open_bench",
        "facts": n,
        "facts_resident": facts,
        "batches": nb,
        "batch_rows": batch_rows,
        "init_ms": init_ms,
        "load_total_ms": load_ms,
        "load_batches": batch_stats,
        "rss_after_load_kb": rss_after_load,
        "disk_bytes": disk_bytes,
        "cold_open": child_json,
        "fallback": fallback,
    });
    println!("{report}");
    if std::env::var_os(KEEP_ENV).is_some() {
        eprintln!("kept bench world at {}", dir.display());
    } else {
        let _ = fs::remove_dir_all(&dir);
    }
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
