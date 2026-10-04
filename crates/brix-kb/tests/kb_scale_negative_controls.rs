//! Negative scale controls for the persistent knowledge base (ADR-0046, P0 stage gate).
//!
//! Demonstrates that the existing brix.kb@1 update path (`ops::assert_inputs`)
//! is strictly O(|world|) in disk bytes written, snapshot serialization, and
//! input validation: asserting 1 single fact re-serializes and re-validates the
//! entire snapshot across all N resident facts.
//!
//! Governed by `docs/planning/persistent-world-runtime-plan.md` §1, §4 (P0).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use brix_kb::ops;
use brix_kb::paths;

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct TempKbDir {
    path: PathBuf,
}

impl TempKbDir {
    fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "brix_kb_p0_negative_{tag}_{}_{n}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempKbDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn o_delta_holds(before: u64, after: u64, tolerance: u64) -> bool {
    after.abs_diff(before) <= tolerance
}

/// Generates a test program with `n` input facts and a single constant decision.
fn generate_kb_program(dir: &Path, n: usize) -> PathBuf {
    let mut src = String::new();
    src.push_str("config Decision = Accept\n\n");
    for i in 0..n {
        src.push_str(&format!("input fact_{i}: Int\n"));
    }
    src.push_str("\npropose accept() priority 1 when true = Accept\n");
    src.push_str("commit result from (accept)\n");

    let path = dir.join("program.brix");
    fs::write(&path, src).unwrap();
    path
}

/// Generates an input shard containing initial values for all `n` input facts.
fn generate_initial_inputs(dir: &Path, n: usize) -> PathBuf {
    let mut json = String::from("{\"schema\":\"brix.input@1\",\"values\":{");
    for i in 0..n {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&format!(
            "\"fact_{i}\":{{\"type\":\"int\",\"value\":\"{i}\"}}"
        ));
    }
    json.push_str("}}");

    let path = dir.join("initial_inputs.json");
    fs::write(&path, json).unwrap();
    path
}

/// Generates a 1-fact update shard updating `fact_0`.
fn generate_single_fact_update(dir: &Path, value: u64) -> PathBuf {
    let json = format!(
        "{{\"schema\":\"brix.input@1\",\"values\":{{\"fact_0\":{{\"type\":\"int\",\"value\":\"{value}\"}}}}}}"
    );
    let path = dir.join(format!("update_{value}.json"));
    fs::write(&path, json).unwrap();
    path
}

struct KbUpdateMetrics {
    snapshot_bytes_written: u64,
    total_resident_facts: usize,
}

fn measure_kb_assert_1_fact(resident_facts: usize) -> KbUpdateMetrics {
    let tmp = TempKbDir::new(&format!("scale_{resident_facts}"));
    let kb_root = tmp.path.join("kb");

    let program_path = generate_kb_program(&tmp.path, resident_facts);
    let inputs_path = generate_initial_inputs(&tmp.path, resident_facts);

    // Initialize KB with `resident_facts`
    let init_outcome =
        ops::init(&kb_root, &program_path, &[inputs_path], &[]).expect("init should succeed");
    assert_eq!(init_outcome.record.seq, 1);

    // Now measure an incremental 1-fact assertion (asserting 1 updated fact into the KB)
    let update_path = generate_single_fact_update(&tmp.path, 42_000);
    let assert_outcome =
        ops::assert_inputs(&kb_root, &[update_path], &[]).expect("assert should succeed");
    assert_eq!(assert_outcome.record.seq, 2);

    // Inspect the size of the written snapshot file for revision 2
    let snapshot_file = paths::snapshot_file(&kb_root, assert_outcome.record.snapshot_id.digest());
    let metadata = fs::metadata(&snapshot_file).unwrap_or_else(|e| {
        panic!(
            "failed to read snapshot file {}: {e}",
            snapshot_file.display()
        )
    });

    KbUpdateMetrics {
        snapshot_bytes_written: metadata.len(),
        total_resident_facts: resident_facts,
    }
}

#[test]
fn negative_control_kb_assert_is_world_proportional_and_fails_scale_gate() {
    let m_25 = measure_kb_assert_1_fact(25);
    let m_50 = measure_kb_assert_1_fact(50);
    let m_100 = measure_kb_assert_1_fact(100);

    // Raw measurements (visible with `--nocapture`): snapshot bytes written by
    // one single-fact `assert` into a KB already holding N facts.
    for m in [&m_25, &m_50, &m_100] {
        eprintln!(
            "kb assert (1 fact) N={:>4}: snapshot file {} bytes",
            m.total_resident_facts, m.snapshot_bytes_written
        );
    }

    // Verifiably, disk bytes written for a 1-fact update scale with the total resident fact count:
    assert!(
        m_50.snapshot_bytes_written > m_25.snapshot_bytes_written,
        "Snapshot size written on 1-fact update must grow with world size (25 facts: {} bytes, 50 facts: {} bytes)",
        m_25.snapshot_bytes_written,
        m_50.snapshot_bytes_written
    );
    assert!(
        m_100.snapshot_bytes_written > m_50.snapshot_bytes_written,
        "Snapshot size written on 1-fact update must grow with world size (50 facts: {} bytes, 100 facts: {} bytes)",
        m_50.snapshot_bytes_written,
        m_100.snapshot_bytes_written
    );

    // The ratio of bytes written scales approximately 2x when doubling facts:
    let ratio_25_to_50 = m_50.snapshot_bytes_written as f64 / m_25.snapshot_bytes_written as f64;
    let ratio_50_to_100 = m_100.snapshot_bytes_written as f64 / m_50.snapshot_bytes_written as f64;
    assert!(
        ratio_25_to_50 > 1.7 && ratio_25_to_50 < 2.3,
        "Expected ~2x growth in snapshot bytes on 2x world growth, got ratio {ratio_25_to_50}"
    );
    assert!(
        ratio_50_to_100 > 1.7 && ratio_50_to_100 < 2.3,
        "Expected ~2x growth in snapshot bytes on 2x world growth, got ratio {ratio_50_to_100}"
    );

    // Must visibly FAIL the flat O(Δ) gate!
    let tolerance_bytes = 64; // A flat update shouldn't vary by more than metadata envelope bytes
    assert!(
        !o_delta_holds(m_50.snapshot_bytes_written, m_100.snapshot_bytes_written, tolerance_bytes),
        "KB 1-fact update was expected to fail O(Δ) gate due to full snapshot serialization, but passed!"
    );
}
