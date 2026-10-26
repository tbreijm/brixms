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

#[test]
fn structured_world_values_survive_stdio_restart_history_and_audit() {
    let scratch = Scratch::new("structured");
    let world = scratch.path().join("world");
    let program = scratch.path().join("atlas.brix");
    fs::write(
        &program,
        r#"
config Rec = { id: Str, amt: Int }
config Decision = Approved(Rec) | Rejected
fn positive(d: Decision): Bool = match d { Approved(r) => r.amt > 0 Rejected => false }
rel input orders: { id: Str, amt: Int, status: Decision } key id
rel derived ready = select { id:o.id, status:o.status } from o in orders where positive(o.status)
decide approve for o in orders per id {
 propose yes priority 10 when o.amt > 0 = Approved(Rec { id:o.id, amt:o.amt })
 propose no priority 100 when true = Rejected
}
decide echo for r in ready per id { propose current priority 1 when true = r.status }
decide record for o in orders per id { propose current priority 1 when true = Rec { id:o.id, amt:o.amt } }
"#,
    )
    .unwrap();
    let approved = json!({"type":"sum","nominal":"atlas::Decision","variant":"Approved","args":[{
        "type":"record","nominal":"atlas::Rec","fields":[
            {"name":"amt","value":{"type":"int","value":"5"}},
            {"name":"id","value":{"type":"string","value":"order-1"}}
        ]
    }]});
    let batch = |revision: u64, amount: i64| {
        json!({
            "schema":"brix.world.batch@1","expected_base_revision":revision,
            "idempotency_key":format!("r1-{revision}"),"operations":[{
                "op":"upsert","relation":"atlas::orders","key":"order-1",
                "tuple":{"id":"order-1","amt":amount,"status":approved}
            }]
        })
    };
    let requests = [
        json!({"id":1,"method":"world.init","params":{"dir":world,"program":{"path":program}}}),
        json!({"id":2,"method":"world.batch","params":{"dir":world,"batch":batch(0,5)}}),
        json!({"id":3,"method":"world.decisions","params":{"dir":world}}),
        json!({"id":4,"method":"world.explain","params":{"dir":world,"entity":"order-1","decide":"atlas::approve"}}),
        json!({"id":5,"method":"world.batch","params":{"dir":world,"batch":batch(1,0)}}),
        json!({"id":6,"method":"world.decisions","params":{"dir":world}}),
        json!({"id":7,"method":"world.explain","params":{"dir":world,"entity":"order-1","decide":"atlas::approve"}}),
    ];
    let mut child = binary()
        .args(["serve", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    for request in requests {
        writeln!(child.stdin.as_mut().unwrap(), "{request}").unwrap();
    }
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let replies: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(replies.len(), 7);
    let candidates = replies[6]["result"]["explanation"]["candidates"]
        .as_array()
        .unwrap();
    assert!(candidates
        .iter()
        .any(|c| c["name"] == "yes" && c["disposition"] == "ineligible"));
    assert!(candidates
        .iter()
        .any(|c| c["name"] == "no" && c["disposition"] == "selected"));
    for reply in &replies {
        assert_eq!(reply["ok"], true, "{reply}");
    }
    assert_eq!(
        replies[2]["result"]["settlements"]["atlas::approve"]["order-1"]["value"],
        approved
    );
    assert_eq!(
        replies[2]["result"]["settlements"]["atlas::echo"]["order-1"]["value"],
        approved
    );
    assert_eq!(
        replies[2]["result"]["settlements"]["atlas::record"]["order-1"]["value"],
        approved["args"][0]
    );
    assert_eq!(
        replies[5]["result"]["settlements"]["atlas::approve"]["order-1"]["value"]["variant"],
        "Rejected"
    );
    // A separate process reloads the journal and historical candidate evidence.
    let historical = parse_stdout(
        binary()
            .args(["world", "explain", "--json"])
            .arg(&world)
            .args(["order-1", "--decide", "atlas::approve", "--rev", "1"])
            .output()
            .unwrap(),
    );
    assert_eq!(historical["explanation"]["value"], approved);
    let current = parse_stdout(
        binary()
            .args(["world", "decide", "--json"])
            .arg(&world)
            .output()
            .unwrap(),
    );
    assert_eq!(
        current["settlements"]["atlas::approve"]["order-1"]["value"]["variant"],
        "Rejected"
    );
    let bundle = scratch.path().join("structured.brixaudit");
    let audit = parse_stdout(
        binary()
            .args(["world", "audit", "--json"])
            .arg(&world)
            .arg("--out")
            .arg(&bundle)
            .output()
            .unwrap(),
    );
    let verified = parse_stdout(
        binary()
            .args(["world", "verify", "--json"])
            .args(["--expect-head", audit["head_digest"].as_str().unwrap()])
            .args([
                "--expect-program",
                replies[0]["result"]["program_digest"].as_str().unwrap(),
            ])
            .arg(&bundle)
            .output()
            .unwrap(),
    );
    assert_eq!(verified["status"], "audited");
    assert_eq!(verified["head_revision"], 2);
}

#[test]
fn test_world_evidence_and_verify_evidence_cli() {
    let scratch = Scratch::new("decision-evidence");
    let program = scratch.path().join("main.brix");
    fs::write(
        &program,
        r#"
config Status = Approved | Rejected
config CertifiedAnswer = { path: Status, caller: Str, release_id: Str, grants: List<Str> }

rel input orders: { id: Str, qty: Int, caller: Str, release_id: Str } key id
rel input order_grants: { id: Str, order_id: Str, grant: Str, ord: Int } key id

rel derived collected_grants = select {
    order_id: g.order_id,
    grants: collect(g.grant, g.ord, 10)
} from g in order_grants group by g.order_id

rel derived order_context = select {
    id: o.id,
    qty: o.qty,
    caller: o.caller,
    release_id: o.release_id,
    grants: cg.grants
} from o in orders, cg in collected_grants
where o.id == cg.order_id

decide review for o in order_context per id {
    propose approved priority 10 when o.qty > 5 = CertifiedAnswer {
        path: Status::Approved,
        caller: o.caller,
        release_id: o.release_id,
        grants: o.grants
    }
    propose rejected priority 20 when true = CertifiedAnswer {
        path: Status::Rejected,
        caller: o.caller,
        release_id: o.release_id,
        grants: o.grants
    }
}
"#,
    )
    .unwrap();

    let world = scratch.path().join("world");
    let init_out = parse_stdout(
        binary()
            .args(["world", "init", "--json"])
            .arg(&world)
            .arg(&program)
            .output()
            .unwrap(),
    );
    let program_digest = init_out["program_digest"].as_str().unwrap();

    let batch = scratch.path().join("batch.json");
    fs::write(
        &batch,
        r#"{
  "schema": "brix.world.batch@1",
  "expected_base_revision": 0,
  "idempotency_key": "b1",
  "operations": [
    {
      "op": "upsert",
      "relation": "main::orders",
      "key": "o-1",
      "tuple": { "id": "o-1", "qty": "10", "caller": "service:orders", "release_id": "atlas-2026.10" }
    },
    {
      "op": "upsert",
      "relation": "main::order_grants",
      "key": "g-1",
      "tuple": { "id": "g-1", "order_id": "o-1", "grant": "read:orders", "ord": "1" }
    },
    {
      "op": "upsert",
      "relation": "main::order_grants",
      "key": "g-2",
      "tuple": { "id": "g-2", "order_id": "o-1", "grant": "role:admin", "ord": "2" }
    }
  ]
}"#,
    )
    .unwrap();

    let batch_out = parse_stdout(
        binary()
            .args(["world", "batch", "--json"])
            .arg(&world)
            .arg(&batch)
            .output()
            .unwrap(),
    );
    let rev = batch_out["revision"].as_u64().unwrap();
    let rev_digest = batch_out["revision_digest"].as_str().unwrap();

    let world_manifest_digest = {
        let session = brix_kb::world::WorldSession::open(&world).unwrap();
        session.manifest().digest().to_hex()
    };

    let evidence_file = scratch.path().join("o1.evidence");
    let ev_out = parse_stdout(
        binary()
            .args(["world", "evidence", "--json"])
            .arg(&world)
            .args(["--rev", &rev.to_string()])
            .args(["--decide", "main::review"])
            .args(["--entity", "o-1"])
            .args(["--caller", "service:orders"])
            .args(["--grant", "read:orders"])
            .args(["--grant", "role:admin"])
            .args(["--out", evidence_file.to_str().unwrap()])
            .output()
            .unwrap(),
    );
    assert_eq!(ev_out["ok"], true);
    assert_eq!(ev_out["settlement"]["candidate_name"], "approved");
    let value_digest = ev_out["target"]["value_digest"].as_str().unwrap();

    // Delete the producer world directory before running fresh-process verify-evidence
    // so portable verification is proven independent of the live world directory.
    fs::remove_dir_all(&world).expect("remove producer world directory");

    let verify_out = parse_stdout(
        binary()
            .args(["world", "verify-evidence", "--json"])
            .args(["--evidence", evidence_file.to_str().unwrap()])
            .args(["--expect-world", &world_manifest_digest])
            .args(["--expect-program", program_digest])
            .args(["--expect-rev", &rev.to_string()])
            .args(["--expect-rev-digest", rev_digest])
            .args(["--expect-decide", "main::review"])
            .args(["--expect-entity", "o-1"])
            .args(["--expect-candidate", "approved"])
            .args(["--expect-value-digest", value_digest])
            .args(["--expect-caller", "service:orders"])
            .args(["--expect-release", "atlas-2026.10"])
            .args(["--expect-grant", "read:orders"])
            .args(["--expect-grant", "role:admin"])
            .output()
            .unwrap(),
    );
    assert_eq!(verify_out["ok"], true);
    assert_eq!(verify_out["receipt"]["subject"], "o-1");
}
