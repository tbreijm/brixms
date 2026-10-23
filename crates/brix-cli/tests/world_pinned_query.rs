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
            "brix-world-pinned-query-{label}-{}-{n}",
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

fn linked_program() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/world-linked/main.brix")
}

#[test]
fn test_revision_pinned_world_query_acceptance() {
    let scratch = Scratch::new("pinned");
    let world = scratch.path().join("world");

    // 1. world init
    let init = parse_stdout(
        binary()
            .args(["world", "init", "--json"])
            .arg(&world)
            .arg(linked_program())
            .output()
            .unwrap(),
    );
    assert_eq!(init["ok"], true);

    // 2. Commit batch 1 (rev 1) with 3 orders
    let batch1 = json!({
        "schema": "brix.world.batch@1",
        "expected_base_revision": 0,
        "idempotency_key": "pinned-seed-1",
        "operations": [
            {"op":"upsert", "relation":"policy::orders", "key":"order-1", "tuple":{"id":"order-1", "customer":"Ada", "sku":"sku-1", "qty":2, "express":"yes"}},
            {"op":"upsert", "relation":"policy::orders", "key":"order-2", "tuple":{"id":"order-2", "customer":"Bob", "sku":"sku-2", "qty":5, "express":"no"}},
            {"op":"upsert", "relation":"policy::orders", "key":"order-3", "tuple":{"id":"order-3", "customer":"Carl", "sku":"sku-3", "qty":1, "express":"yes"}},
            {"op":"upsert", "relation":"policy::inventory", "key":"inventory-1", "tuple":{"id":"inventory-1", "sku":"sku-1", "available":9}},
            {"op":"upsert", "relation":"policy::shipping", "key":"shipping-1", "tuple":{"id":"shipping-1", "sku":"sku-1", "carrier":"ParcelCo"}}
        ]
    });
    let batch1_file = scratch.path().join("batch1.json");
    fs::write(&batch1_file, serde_json::to_vec(&batch1).unwrap()).unwrap();
    let applied1 = parse_stdout(
        binary()
            .args(["world", "batch", "--json"])
            .arg(&world)
            .arg(&batch1_file)
            .output()
            .unwrap(),
    );
    assert_eq!(applied1["ok"], true);
    assert_eq!(applied1["revision"], 1);
    let rev1_digest = applied1["revision_digest"].as_str().unwrap().to_string();

    // 3. Read page one at N (limit 2)
    let page1 = parse_stdout(
        binary()
            .args(["world", "query", "--json"])
            .arg(&world)
            .args(["policy::orders", "--limit", "2"])
            .output()
            .unwrap(),
    );
    assert_eq!(page1["ok"], true);
    assert_eq!(page1["revision"], 1);
    assert_eq!(page1["revision_digest"], rev1_digest);
    assert!(page1["world"].as_str().is_some());
    assert!(page1["program"].as_str().is_some());
    assert_eq!(page1["count"], 2);
    assert_eq!(page1["has_more"], true);
    let cursor_rev1 = page1["next_cursor"].as_str().unwrap().to_string();

    // Also test --typed flag
    let typed_page = parse_stdout(
        binary()
            .args(["world", "query", "--json", "--typed"])
            .arg(&world)
            .args(["policy::orders", "--limit", "1"])
            .output()
            .unwrap(),
    );
    assert_eq!(typed_page["ok"], true);
    // In typed output, qty should be an integer, not string
    assert!(typed_page["entries"][0]["tuple"]["qty"].is_number());

    // 4. Commit N+1 (revision 2) with order-4 and order-5
    let batch2 = json!({
        "schema": "brix.world.batch@1",
        "expected_base_revision": 1,
        "idempotency_key": "pinned-seed-2",
        "operations": [
            {"op":"upsert", "relation":"policy::orders", "key":"order-4", "tuple":{"id":"order-4", "customer":"Dan", "sku":"sku-4", "qty":3, "express":"no"}},
            {"op":"upsert", "relation":"policy::orders", "key":"order-5", "tuple":{"id":"order-5", "customer":"Eve", "sku":"sku-5", "qty":4, "express":"yes"}}
        ]
    });
    let batch2_file = scratch.path().join("batch2.json");
    fs::write(&batch2_file, serde_json::to_vec(&batch2).unwrap()).unwrap();
    let applied2 = parse_stdout(
        binary()
            .args(["world", "batch", "--json"])
            .arg(&world)
            .arg(&batch2_file)
            .output()
            .unwrap(),
    );
    assert_eq!(applied2["ok"], true);
    assert_eq!(applied2["revision"], 2);
    let rev2_digest = applied2["revision_digest"].as_str().unwrap().to_string();
    assert_ne!(rev1_digest, rev2_digest);

    // 5. Mismatched cursor without --rev: resuming at HEAD (N+1) with cursor from N must be refused!
    let stale_resume = binary()
        .args(["world", "query", "--json"])
        .arg(&world)
        .args(["policy::orders", "--cursor", &cursor_rev1, "--limit", "2"])
        .output()
        .unwrap();
    let stale_json: Value = serde_json::from_slice(&stale_resume.stdout).unwrap();
    assert_eq!(stale_json["ok"], false);
    assert_eq!(stale_json["status"], "invalid-cursor");

    // 6. Complete original pagination at N (passing --rev 1)
    let page2_at_rev1 = parse_stdout(
        binary()
            .args(["world", "query", "--json"])
            .arg(&world)
            .args(["policy::orders", "--rev", "1", "--cursor", &cursor_rev1, "--limit", "2"])
            .output()
            .unwrap(),
    );
    assert_eq!(page2_at_rev1["ok"], true);
    assert_eq!(page2_at_rev1["revision"], 1);
    assert_eq!(page2_at_rev1["revision_digest"], rev1_digest);
    assert_eq!(page2_at_rev1["count"], 1);
    assert_eq!(page2_at_rev1["has_more"], false);

    // Collect all entries across both pages at rev 1
    let mut all_rev1_keys: Vec<String> = page1["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["tuple"]["id"].as_str().unwrap().to_string())
        .collect();
    all_rev1_keys.extend(
        page2_at_rev1["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["tuple"]["id"].as_str().unwrap().to_string()),
    );
    all_rev1_keys.sort();
    assert_eq!(all_rev1_keys, vec!["order-1", "order-2", "order-3"]);

    // 7. Verify --expect-digest: correct digest succeeds, wrong digest is refused
    let correct_digest_query = parse_stdout(
        binary()
            .args(["world", "query", "--json"])
            .arg(&world)
            .args(["policy::orders", "--rev", "1", "--expect-digest", &rev1_digest])
            .output()
            .unwrap(),
    );
    assert_eq!(correct_digest_query["ok"], true);

    let wrong_digest = "0000000000000000000000000000000000000000000000000000000000000000";
    let wrong_digest_out = binary()
        .args(["world", "query", "--json"])
        .arg(&world)
        .args(["policy::orders", "--rev", "1", "--expect-digest", wrong_digest])
        .output()
        .unwrap();
    let wrong_json: Value = serde_json::from_slice(&wrong_digest_out.stdout).unwrap();
    assert_eq!(wrong_json["ok"], false);
    assert_eq!(wrong_json["status"], "revision-digest-mismatch");

    // 8. Test serve (--stdio) agreement
    let serve_requests = [
        json!({
            "id": 1,
            "method": "world.query",
            "params": {
                "dir": &world,
                "relation": "policy::orders",
                "rev": 1,
                "limit": 2
            }
        }),
        json!({
            "id": 2,
            "method": "world.query",
            "params": {
                "dir": &world,
                "relation": "policy::orders",
                "rev": 1,
                "expect_digest": &rev1_digest
            }
        }),
        json!({
            "id": 3,
            "method": "world.query",
            "params": {
                "dir": &world,
                "relation": "policy::orders",
                "rev": 1,
                "expect_digest": wrong_digest
            }
        })
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
        for req in serve_requests {
            writeln!(stdin, "{}", serde_json::to_string(&req).unwrap()).unwrap();
        }
    }
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let lines: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["result"]["ok"], true);
    assert_eq!(lines[0]["result"]["count"], 2);
    assert_eq!(lines[0]["result"]["revision"], 1);

    assert_eq!(lines[1]["result"]["ok"], true);
    assert_eq!(lines[1]["result"]["revision_digest"], rev1_digest);

    assert_eq!(lines[2]["result"]["ok"], false);
    assert_eq!(lines[2]["result"]["status"], "revision-digest-mismatch");
}
