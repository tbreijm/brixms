use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "brix-world-public-{label}-{}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn brix_bin() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_brix")
        .or_else(|| std::env::var_os("NEXTEST_BIN_EXE_brix"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_brix")))
}

fn binary() -> Command {
    Command::new(brix_bin())
}

fn parse_stdout(output: std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "status: {}; stdout: {}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "invalid JSON stdout ({e}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn batch() -> Value {
    json!({
        "schema": "brix.world.batch@1",
        "expected_base_revision": 0,
        "idempotency_key": "world-public-seed-1",
        "operations": [
            {"op":"upsert", "relation":"policy::orders", "key":"order-1", "tuple":{"id":"order-1", "customer":"Ada", "sku":"sku-1", "qty":2, "express":"yes"}},
            {"op":"upsert", "relation":"policy::inventory", "key":"inventory-1", "tuple":{"id":"inventory-1", "sku":"sku-1", "available":9}},
            {"op":"upsert", "relation":"policy::shipping", "key":"shipping-1", "tuple":{"id":"shipping-1", "sku":"sku-1", "carrier":"ParcelCo"}}
        ]
    })
}

fn linked_program() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/world-linked/main.brix")
}

#[test]
fn binary_world_lifecycle_uses_structured_tuples_and_decisions() {
    let scratch = Scratch::new("cli");
    let world = scratch.path().join("world");
    let init = parse_stdout(
        binary()
            .args(["world", "init", "--json"])
            .arg(&world)
            .arg(linked_program())
            .output()
            .unwrap(),
    );
    assert_eq!(init["command"], "world init");
    assert_eq!(init["ok"], true);
    assert!(init["relations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r == "policy::orders"));

    let batch_file = scratch.path().join("batch.json");
    fs::write(&batch_file, serde_json::to_vec(&batch()).unwrap()).unwrap();
    let applied = parse_stdout(
        binary()
            .args(["world", "batch", "--json"])
            .arg(&world)
            .arg(&batch_file)
            .output()
            .unwrap(),
    );
    assert_eq!(applied["command"], "world batch");
    assert_eq!(applied["ok"], true);
    assert_eq!(applied["revision"], 1);
    assert_eq!(applied["changed_keys_count"], 3);
    assert_eq!(applied["is_idempotent_replay"], false);

    // Cached prior results must be distinguishable from a freshly committed
    // batch (ADR-0046 §4 P5 bullet): resubmitting the identical batch (same
    // idempotency key, same payload) through the same public CLI path
    // returns the cached receipt rather than re-deliberating.
    let replay_json = parse_stdout(
        binary()
            .args(["world", "batch", "--json"])
            .arg(&world)
            .arg(&batch_file)
            .output()
            .unwrap(),
    );
    assert_eq!(replay_json["ok"], true);
    assert_eq!(replay_json["revision"], 1, "replay must not advance HEAD");
    assert_eq!(replay_json["is_idempotent_replay"], true);
    let replay_human = binary()
        .args(["world", "batch"])
        .arg(&world)
        .arg(&batch_file)
        .output()
        .unwrap();
    let replay_stdout = String::from_utf8_lossy(&replay_human.stdout);
    assert!(
        replay_stdout.contains("Idempotent replay"),
        "human output must distinguish a cached replay from a fresh commit: {replay_stdout}"
    );

    let query = parse_stdout(
        binary()
            .args(["world", "query", "--json"])
            .arg(&world)
            .args(["policy::orders"])
            .output()
            .unwrap(),
    );
    assert_eq!(query["ok"], true);
    assert_eq!(query["count"], 1);
    assert_eq!(query["entries"][0]["tuple"]["id"], "order-1");
    assert_eq!(query["entries"][0]["tuple"]["qty"], "2");

    let decisions = parse_stdout(
        binary()
            .args(["world", "decide", "--json"])
            .arg(&world)
            .output()
            .unwrap(),
    );
    assert_eq!(decisions["ok"], true);
    assert!(!decisions["settlements"]["main::dispatch"]
        .as_object()
        .unwrap()
        .is_empty());
}

#[test]
fn binary_world_init_fails_closed_for_missing_or_corrupt_program_input() {
    let scratch = Scratch::new("init-errors");
    let missing_world = scratch.path().join("missing-world");
    let missing = binary()
        .args(["world", "init", "--json"])
        .arg(&missing_world)
        .arg(scratch.path().join("absent.brix"))
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
    let missing_json: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(missing_json["ok"], false);
    assert!(!missing_world.exists());

    let bad_program = scratch.path().join("bad.brix");
    fs::write(&bad_program, "use missing_module\n").unwrap();
    let corrupt_world = scratch.path().join("corrupt-world");
    let corrupt = binary()
        .args(["world", "init", "--json"])
        .arg(&corrupt_world)
        .arg(&bad_program)
        .output()
        .unwrap();
    assert_ne!(corrupt.status.code(), Some(0));
    let corrupt_json: Value = serde_json::from_slice(&corrupt.stdout).unwrap();
    assert_eq!(corrupt_json["ok"], false);
    assert!(!corrupt_world.exists());
    // A missing import (an unresolvable dependency) is a missing input, not a
    // generic network error: the two failure json's `status` must differ,
    // each distinguishing its own category (ADR-0046 §4 P5 bullet).
    assert_eq!(
        corrupt_json["status"], "missing-input",
        "unresolved import must report status 'missing-input': {corrupt_json}"
    );
}

/// ADR-0046 §4 P5 requires missing inputs, unsupported operators, exhaustion,
/// and cached prior results to be distinguishable in human and JSON output.
/// Cached-replay is covered by `binary_world_lifecycle_uses_structured_tuples_and_decisions`'s
/// `is_idempotent_replay` assertion; this test covers the other three
/// failure categories through the real CLI binary.
#[test]
fn binary_world_errors_distinguish_status_categories() {
    let scratch = Scratch::new("status-categories");

    // Unsupported operator: a recursive relation cycle is rejected at
    // lowering (ADR-0046 §3.5), not silently miscounted as a network error.
    let cyclic = scratch.path().join("cyclic.brix");
    fs::write(
        &cyclic,
        "rel input orders: { id: Str } key id\n\
         rel derived a = select { id: x.id } from x in b\n\
         rel derived b = select { id: y.id } from y in a\n",
    )
    .unwrap();
    let cyclic_out = binary()
        .args(["world", "init", "--json"])
        .arg(scratch.path().join("cyclic-world"))
        .arg(&cyclic)
        .output()
        .unwrap();
    let cyclic_json: Value = serde_json::from_slice(&cyclic_out.stdout).unwrap();
    assert_eq!(cyclic_json["ok"], false);
    assert_eq!(
        cyclic_json["status"], "unsupported-operator",
        "recursive relation cycle must report status 'unsupported-operator': {cyclic_json}"
    );
    // The human-readable (non-JSON) path carries the same distinguishing
    // status word, not just the JSON path.
    let cyclic_human = binary()
        .args(["world", "init"])
        .arg(scratch.path().join("cyclic-world-human"))
        .arg(&cyclic)
        .output()
        .unwrap();
    let cyclic_stderr = String::from_utf8_lossy(&cyclic_human.stderr);
    assert!(
        cyclic_stderr.contains("unsupported-operator"),
        "human output must name the status category: {cyclic_stderr}"
    );

    // Resource exhaustion: a single module exceeding the bounded-loader byte
    // limit is refused before unbounded allocation (ADR-0046 §3.4/§3.8), and
    // must not collapse into the same bucket as a missing input.
    let oversized = scratch.path().join("oversized.brix");
    let mut src = String::from("rel input orders: { id: Str } key id\n//");
    src.push_str(&"x".repeat(1_100_000));
    fs::write(&oversized, src).unwrap();
    let oversized_out = binary()
        .args(["world", "init", "--json"])
        .arg(scratch.path().join("oversized-world"))
        .arg(&oversized)
        .output()
        .unwrap();
    let oversized_json: Value = serde_json::from_slice(&oversized_out.stdout).unwrap();
    assert_eq!(oversized_json["ok"], false);
    assert_eq!(
        oversized_json["status"], "resource-exhaustion",
        "module exceeding the byte limit must report status 'resource-exhaustion': {oversized_json}"
    );

    // Missing input: an unresolvable import is distinct from both of the above.
    let missing_import = scratch.path().join("missing-import.brix");
    fs::write(&missing_import, "use nonexistent_module\n").unwrap();
    let missing_import_out = binary()
        .args(["world", "init", "--json"])
        .arg(scratch.path().join("missing-import-world"))
        .arg(&missing_import)
        .output()
        .unwrap();
    let missing_import_json: Value = serde_json::from_slice(&missing_import_out.stdout).unwrap();
    assert_eq!(missing_import_json["ok"], false);
    assert_eq!(missing_import_json["status"], "missing-input");

    // All three statuses observed above must be pairwise distinct.
    let statuses = [
        cyclic_json["status"].as_str().unwrap(),
        oversized_json["status"].as_str().unwrap(),
        missing_import_json["status"].as_str().unwrap(),
    ];
    assert_eq!(
        statuses.iter().collect::<std::collections::BTreeSet<_>>().len(),
        3,
        "unsupported-operator / resource-exhaustion / missing-input must be pairwise distinct: {statuses:?}"
    );
}

#[test]
fn stdio_world_methods_dispatch_real_lifecycle_calls() {
    let scratch = Scratch::new("stdio");
    let world = scratch.path().join("world");
    let requests = [
        json!({"id":1,"method":"world.init","params":{"dir":world,"program":{"path":linked_program()}}}),
        json!({"id":2,"method":"world.batch","params":{"dir":world,"batch":batch()}}),
        json!({"id":3,"method":"world.query","params":{"dir":world,"relation":"policy::orders"}}),
        json!({"id":4,"method":"world.decisions","params":{"dir":world,"decide":"main::dispatch"}}),
        json!({"id":5,"method":"world.explain","params":{"dir":world,"entity":"order-1","decide":"main::dispatch"}}),
    ];
    let mut child = binary()
        .args(["serve", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        for request in requests {
            writeln!(stdin, "{}", serde_json::to_string(&request).unwrap()).unwrap();
        }
    }
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 5);
    for (index, line) in lines.iter().enumerate() {
        assert_eq!(line["id"], index as u64 + 1);
        assert_eq!(line["ok"], true, "response {line}");
        assert_eq!(line["exit_code"], 0, "response {line}");
    }
    assert_eq!(lines[2]["result"]["entries"][0]["tuple"]["customer"], "Ada");
    assert!(!lines[3]["result"]["settlements"]["main::dispatch"]
        .as_object()
        .unwrap()
        .is_empty());
}
