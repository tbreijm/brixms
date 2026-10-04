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

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_brix"))
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
    assert!(!decisions["settlements"]["policy::dispatch"]
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
}

#[test]
fn stdio_world_methods_dispatch_real_lifecycle_calls() {
    let scratch = Scratch::new("stdio");
    let world = scratch.path().join("world");
    let requests = [
        json!({"id":1,"method":"world.init","params":{"dir":world,"program":{"path":linked_program()}}}),
        json!({"id":2,"method":"world.batch","params":{"dir":world,"batch":batch()}}),
        json!({"id":3,"method":"world.query","params":{"dir":world,"relation":"policy::orders"}}),
        json!({"id":4,"method":"world.decisions","params":{"dir":world,"decide":"policy::dispatch"}}),
        json!({"id":5,"method":"world.explain","params":{"dir":world,"entity":"order-1","decide":"policy::dispatch"}}),
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
    assert!(!lines[3]["result"]["settlements"]["policy::dispatch"]
        .as_object()
        .unwrap()
        .is_empty());
}
