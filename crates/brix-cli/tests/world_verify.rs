//! CLI integration tests for `brix world audit`, `brix world verify`, and `brix world import-kb`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{json, Value};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "brix-cli-verify-{label}-{}-{n}",
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

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn linked_program() -> PathBuf {
    repo_root().join("examples/world-linked/main.brix")
}

fn parse_stdout(output: Output) -> Value {
    assert!(
        output.status.success(),
        "status: {}; stdout: {}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn parse_output_json(output: Output) -> (i32, Value) {
    let code = output.status.code().unwrap_or(-1);
    let val: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "invalid JSON stdout ({e}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    (code, val)
}

fn batch1() -> Value {
    json!({
        "schema": "brix.world.batch@1",
        "expected_base_revision": 0,
        "idempotency_key": "verify-test-batch-1",
        "operations": [
            {"op":"upsert", "relation":"policy::orders", "key":"order-1", "tuple":{"id":"order-1", "customer":"Ada", "sku":"sku-1", "qty":2, "express":"yes"}},
            {"op":"upsert", "relation":"policy::inventory", "key":"inventory-1", "tuple":{"id":"inventory-1", "sku":"sku-1", "available":9}},
            {"op":"upsert", "relation":"policy::shipping", "key":"shipping-1", "tuple":{"id":"shipping-1", "sku":"sku-1", "carrier":"ParcelCo"}}
        ]
    })
}

fn batch2() -> Value {
    json!({
        "schema": "brix.world.batch@1",
        "expected_base_revision": 1,
        "idempotency_key": "verify-test-batch-2",
        "operations": [
            {"op":"upsert", "relation":"policy::orders", "key":"order-2", "tuple":{"id":"order-2", "customer":"Grace", "sku":"sku-1", "qty":1, "express":"no"}}
        ]
    })
}

#[test]
fn test_cli_world_audit_and_verify_bundle_roundtrip() {
    let scratch = Scratch::new("roundtrip");
    let world_dir = scratch.path().join("world");

    // 1. world init
    let init = parse_stdout(
        binary()
            .args(["world", "init", "--json"])
            .arg(&world_dir)
            .arg(linked_program())
            .output()
            .unwrap(),
    );
    assert_eq!(init["ok"], true);
    let program_digest = init["program_digest"].as_str().unwrap().to_string();

    // 2. world batch
    let batch_file = scratch.path().join("batch1.json");
    fs::write(&batch_file, serde_json::to_vec(&batch1()).unwrap()).unwrap();
    let batch_res = parse_stdout(
        binary()
            .args(["world", "batch", "--json"])
            .arg(&world_dir)
            .arg(&batch_file)
            .output()
            .unwrap(),
    );
    assert_eq!(batch_res["ok"], true);
    assert_eq!(batch_res["revision"], 1);
    let head_digest = batch_res["digest"].as_str().unwrap().to_string();

    // 3. world audit
    let bundle_file = scratch.path().join("bundle.brixaudit");
    let audit_res = parse_stdout(
        binary()
            .args(["world", "audit", "--json"])
            .arg(&world_dir)
            .args(["--out", bundle_file.to_str().unwrap()])
            .output()
            .unwrap(),
    );
    assert_eq!(audit_res["ok"], true);
    assert_eq!(audit_res["head_seq"], 1);
    assert_eq!(audit_res["head_digest"], head_digest);
    assert_eq!(audit_res["scope"], "complete-from-genesis");
    assert!(bundle_file.exists());

    // 4. world verify bundle
    let verify_res = parse_stdout(
        binary()
            .args(["world", "verify", "--json"])
            .args(["--expect-head", &head_digest])
            .args(["--expect-program", &program_digest])
            .arg(&bundle_file)
            .output()
            .unwrap(),
    );
    assert_eq!(verify_res["ok"], true);
    assert_eq!(verify_res["status"], "audited");
    assert_eq!(verify_res["head_revision"], 1);
    assert_eq!(verify_res["head_digest"], head_digest);
    assert_eq!(verify_res["program_digest"], program_digest);
    assert!(verify_res["work"]["tuples_decoded"].as_u64().unwrap() >= 3);
    assert!(verify_res["work"]["settlements_computed"].as_u64().unwrap() >= 1);
    let settlements = verify_res["settlements"].as_array().unwrap();
    assert!(!settlements.is_empty());
    assert_eq!(settlements[0]["status"], "audited");
}

#[test]
fn test_cli_world_verify_in_place() {
    let scratch = Scratch::new("in_place");
    let world_dir = scratch.path().join("world");

    let init = parse_stdout(
        binary()
            .args(["world", "init", "--json"])
            .arg(&world_dir)
            .arg(linked_program())
            .output()
            .unwrap(),
    );
    let program_digest = init["program_digest"].as_str().unwrap().to_string();

    let batch_file = scratch.path().join("batch1.json");
    fs::write(&batch_file, serde_json::to_vec(&batch1()).unwrap()).unwrap();
    let batch_res = parse_stdout(
        binary()
            .args(["world", "batch", "--json"])
            .arg(&world_dir)
            .arg(&batch_file)
            .output()
            .unwrap(),
    );
    let head_digest = batch_res["digest"].as_str().unwrap().to_string();

    // Verify in-place without creating an on-disk bundle file
    let verify_res = parse_stdout(
        binary()
            .args(["world", "verify", "--json"])
            .args(["--expect-head", &head_digest])
            .args(["--expect-program", &program_digest])
            .args(["--in-place", world_dir.to_str().unwrap()])
            .output()
            .unwrap(),
    );
    assert_eq!(verify_res["ok"], true);
    assert_eq!(verify_res["status"], "audited");
    assert_eq!(verify_res["head_revision"], 1);
}

#[test]
fn test_cli_world_verify_pin_mismatch_refusal() {
    let scratch = Scratch::new("pin_refusal");
    let world_dir = scratch.path().join("world");

    let init = parse_stdout(
        binary()
            .args(["world", "init", "--json"])
            .arg(&world_dir)
            .arg(linked_program())
            .output()
            .unwrap(),
    );
    let program_digest = init["program_digest"].as_str().unwrap().to_string();

    let batch_file = scratch.path().join("batch1.json");
    fs::write(&batch_file, serde_json::to_vec(&batch1()).unwrap()).unwrap();
    let batch_res = parse_stdout(
        binary()
            .args(["world", "batch", "--json"])
            .arg(&world_dir)
            .arg(&batch_file)
            .output()
            .unwrap(),
    );
    let head_digest = batch_res["digest"].as_str().unwrap().to_string();

    let bundle_file = scratch.path().join("bundle.brixaudit");
    let _ = parse_stdout(
        binary()
            .args(["world", "audit", "--json"])
            .arg(&world_dir)
            .args(["--out", bundle_file.to_str().unwrap()])
            .output()
            .unwrap(),
    );

    let wrong_digest = "0000000000000000000000000000000000000000000000000000000000000000";

    // Wrong head pin -> refuses
    let (code1, res1) = parse_output_json(
        binary()
            .args(["world", "verify", "--json"])
            .args(["--expect-head", wrong_digest])
            .args(["--expect-program", &program_digest])
            .arg(&bundle_file)
            .output()
            .unwrap(),
    );
    assert_eq!(code1, 1);
    assert_eq!(res1["ok"], false);
    assert_eq!(res1["status"], "unknown");
    assert!(res1["errors"][0]
        .as_str()
        .unwrap()
        .contains("head-pin-mismatch"));

    // Wrong program pin -> refuses
    let (code2, res2) = parse_output_json(
        binary()
            .args(["world", "verify", "--json"])
            .args(["--expect-head", &head_digest])
            .args(["--expect-program", wrong_digest])
            .arg(&bundle_file)
            .output()
            .unwrap(),
    );
    assert_eq!(code2, 1);
    assert_eq!(res2["ok"], false);
    assert_eq!(res2["status"], "unknown");
    assert!(res2["errors"][0]
        .as_str()
        .unwrap()
        .contains("program-pin-mismatch"));
}

#[test]
fn test_cli_world_audit_and_verify_checkpoint_suffix() {
    let scratch = Scratch::new("checkpoint");
    let world_dir = scratch.path().join("world");

    let init = parse_stdout(
        binary()
            .args(["world", "init", "--json"])
            .arg(&world_dir)
            .arg(linked_program())
            .output()
            .unwrap(),
    );
    let program_digest = init["program_digest"].as_str().unwrap().to_string();

    let batch1_file = scratch.path().join("batch1.json");
    fs::write(&batch1_file, serde_json::to_vec(&batch1()).unwrap()).unwrap();
    let batch1_res = parse_stdout(
        binary()
            .args(["world", "batch", "--json"])
            .arg(&world_dir)
            .arg(&batch1_file)
            .output()
            .unwrap(),
    );
    let head1_digest = batch1_res["digest"].as_str().unwrap().to_string();

    let batch2_file = scratch.path().join("batch2.json");
    fs::write(&batch2_file, serde_json::to_vec(&batch2()).unwrap()).unwrap();
    let batch2_res = parse_stdout(
        binary()
            .args(["world", "batch", "--json"])
            .arg(&world_dir)
            .arg(&batch2_file)
            .output()
            .unwrap(),
    );
    let head2_digest = batch2_res["digest"].as_str().unwrap().to_string();

    // Export checkpoint-scoped bundle from revision 1
    let cp_bundle_file = scratch.path().join("checkpoint_bundle.brixaudit");
    let audit_res = parse_stdout(
        binary()
            .args(["world", "audit", "--json"])
            .arg(&world_dir)
            .args(["--out", cp_bundle_file.to_str().unwrap()])
            .args(["--from-checkpoint", "1"])
            .output()
            .unwrap(),
    );
    assert_eq!(audit_res["ok"], true);
    assert_eq!(audit_res["head_seq"], 2);
    assert_eq!(audit_res["scope"], "checkpoint-suffix (1..HEAD)");

    // Verify checkpoint bundle with trust pin
    let verify_res = parse_stdout(
        binary()
            .args(["world", "verify", "--json"])
            .args(["--expect-head", &head2_digest])
            .args(["--expect-program", &program_digest])
            .args(["--trust-checkpoint", &head1_digest])
            .arg(&cp_bundle_file)
            .output()
            .unwrap(),
    );
    assert_eq!(verify_res["ok"], true);
    assert_eq!(verify_res["status"], "audited");
    assert_eq!(verify_res["head_revision"], 2);
    assert!(verify_res["scope"]
        .as_str()
        .unwrap()
        .contains("checkpoint-suffix (1..HEAD)"));
}

#[test]
fn test_cli_world_import_kb_lifecycle() {
    let scratch = Scratch::new("import_kb");
    let kb_dir = scratch.path().join("kb");
    let world_dir = scratch.path().join("world");

    // Initialize KB v1
    let kb_init_out = binary()
        .current_dir(repo_root())
        .arg("kb")
        .arg("init")
        .arg(&kb_dir)
        .arg("examples/shipping-input.brix")
        .arg("--input")
        .arg("examples/shipping-input.json")
        .output()
        .unwrap();
    assert!(kb_init_out.status.success());

    // Import KB into world
    let import_res = parse_stdout(
        binary()
            .current_dir(repo_root())
            .args(["world", "import-kb", "--json"])
            .arg(&kb_dir)
            .arg(&world_dir)
            .output()
            .unwrap(),
    );
    assert_eq!(import_res["ok"], true);
    assert_eq!(import_res["command"], "world import-kb");
    assert_eq!(import_res["kb_head_seq"], 1);
    assert_eq!(import_res["world_head_seq"], 1);

    // Provenance file was written
    let prov_path = PathBuf::from(import_res["provenance_path"].as_str().unwrap());
    assert!(prov_path.exists());

    // World can be inspected via world show
    let show_res = parse_stdout(
        binary()
            .args(["world", "show", "--json"])
            .arg(&world_dir)
            .args(["--rev", "1"])
            .output()
            .unwrap(),
    );
    assert_eq!(show_res["ok"], true);
    assert_eq!(show_res["revision"]["seq"], 1);

    // Re-importing into existing world directory is refused
    let (err_code, err_res) = parse_output_json(
        binary()
            .args(["world", "import-kb", "--json"])
            .arg(&kb_dir)
            .arg(&world_dir)
            .output()
            .unwrap(),
    );
    assert_eq!(err_code, 1);
    assert_eq!(err_res["ok"], false);
    assert_eq!(err_res["status"], "unknown");
}

#[test]
fn test_cli_r5_repro_qualified_nominal_long_name_commits_settles_and_verifies() {
    let scratch = Scratch::new("r5_cli");
    let world_dir = scratch.path().join("world");

    let p_brix = scratch.path().join("p.brix");
    let p_brix_content = r#"
config Mxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx = { amt: Int }
config Out = Ok(Mxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx) | No
rel input orders: { id: Str, m: Mxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx } key id
decide d for o in orders per id {
  propose ok priority 1 when o.m.amt > 0 = Ok(o.m)
  propose no priority 2 when true = No
}
"#;
    fs::write(&p_brix, p_brix_content).unwrap();

    // 1. world init
    let init_res = parse_stdout(
        binary()
            .args(["world", "init", "--json"])
            .arg(&world_dir)
            .arg(&p_brix)
            .output()
            .unwrap(),
    );
    assert_eq!(init_res["ok"], true);
    let program_digest = init_res["program_digest"].as_str().unwrap().to_string();

    // 2. world batch with 65-byte qualified nominal
    let batch_file = scratch.path().join("b.json");
    let batch_content = json!({
        "schema": "brix.world.batch@1",
        "expected_base_revision": 0,
        "idempotency_key": "k",
        "operations": [
            {
                "op": "upsert",
                "relation": "p::orders",
                "key": "o1",
                "tuple": {
                    "id": "o1",
                    "m": {
                        "type": "record",
                        "nominal": "p::Mxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
                        "fields": [
                            {"name": "amt", "value": {"type": "int", "value": "5"}}
                        ]
                    }
                }
            }
        ]
    });
    fs::write(&batch_file, serde_json::to_vec(&batch_content).unwrap()).unwrap();

    let batch_res = parse_stdout(
        binary()
            .args(["world", "batch", "--json"])
            .arg(&world_dir)
            .arg(&batch_file)
            .output()
            .unwrap(),
    );
    assert_eq!(batch_res["ok"], true);
    assert_eq!(batch_res["revision"], 1);
    let head_digest = batch_res["digest"].as_str().unwrap().to_string();

    // 3. world decide
    let decide_res = parse_stdout(
        binary()
            .args(["world", "decide", "--json"])
            .arg(&world_dir)
            .output()
            .unwrap(),
    );
    assert_eq!(decide_res["ok"], true);
    let settlement = &decide_res["settlements"]["p::d"]["o1"];
    assert_eq!(settlement["candidate_name"], "ok");
    assert_eq!(settlement["value"]["type"], "sum");
    assert_eq!(settlement["value"]["nominal"], "p::Out");
    assert_eq!(settlement["value"]["variant"], "Ok");
    assert_eq!(
        settlement["value"]["args"][0]["nominal"],
        "p::Mxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
    );
    assert_eq!(
        settlement["value"]["args"][0]["fields"][0]["value"]["value"],
        "5"
    );

    // 4. world audit
    let bundle_file = scratch.path().join("bundle.brixaudit");
    let audit_res = parse_stdout(
        binary()
            .args(["world", "audit", "--json"])
            .arg(&world_dir)
            .args(["--out", bundle_file.to_str().unwrap()])
            .output()
            .unwrap(),
    );
    assert_eq!(audit_res["ok"], true);
    assert_eq!(audit_res["head_seq"], 1);
    assert_eq!(audit_res["head_digest"], head_digest);
    assert!(bundle_file.exists());

    // 5. world verify
    let verify_res = parse_stdout(
        binary()
            .args(["world", "verify", "--json"])
            .args(["--expect-head", &head_digest])
            .args(["--expect-program", &program_digest])
            .arg(&bundle_file)
            .output()
            .unwrap(),
    );
    assert_eq!(verify_res["ok"], true);
    assert_eq!(verify_res["status"], "audited");
    assert_eq!(verify_res["head_revision"], 1);

    // 6. Test rejection of segment > 64 bytes via CLI
    let seg_65 = "M".repeat(65);
    let bad_batch_file = scratch.path().join("bad_batch.json");
    let bad_batch_content = json!({
        "schema": "brix.world.batch@1",
        "expected_base_revision": 1,
        "idempotency_key": "k2",
        "operations": [
            {
                "op": "upsert",
                "relation": "p::orders",
                "key": "o2",
                "tuple": {
                    "id": "o2",
                    "m": {
                        "type": "record",
                        "nominal": format!("p::{seg_65}"),
                        "fields": [
                            {"name": "amt", "value": {"type": "int", "value": "5"}}
                        ]
                    }
                }
            }
        ]
    });
    fs::write(
        &bad_batch_file,
        serde_json::to_vec(&bad_batch_content).unwrap(),
    )
    .unwrap();

    let (bad_code, bad_res) = parse_output_json(
        binary()
            .args(["world", "batch", "--json"])
            .arg(&world_dir)
            .arg(&bad_batch_file)
            .output()
            .unwrap(),
    );
    assert_ne!(bad_code, 0);
    assert_eq!(bad_res["ok"], false);
    let err_msg = bad_res["errors"][0].as_str().unwrap();
    assert!(
        err_msg.contains("p::orders[o2].m"),
        "error message must include relation/key/field: {err_msg}"
    );
    assert!(
        err_msg.contains(&format!("p::{seg_65}")),
        "error message must name nominal: {err_msg}"
    );
    assert!(
        err_msg.contains(&seg_65),
        "error message must name segment: {err_msg}"
    );
    assert!(
        !err_msg.contains("string value length"),
        "error message must not say string value length: {err_msg}"
    );

    // Also verify non-JSON output format directly
    let raw_out = binary()
        .args(["world", "batch"])
        .arg(&world_dir)
        .arg(&bad_batch_file)
        .output()
        .unwrap();
    assert!(!raw_out.status.success());
    let stderr_str = String::from_utf8_lossy(&raw_out.stderr);
    assert!(
        stderr_str.contains("p::orders[o2].m"),
        "stderr must include relation/key/field: {stderr_str}"
    );
    assert!(
        stderr_str.contains(&format!("p::{seg_65}")),
        "stderr must name nominal: {stderr_str}"
    );
    assert!(
        stderr_str.contains(&seg_65),
        "stderr must name segment: {stderr_str}"
    );
    assert!(
        !stderr_str.contains("string value length"),
        "stderr must not say string value length: {stderr_str}"
    );
}
