//! Comprehensive binary integration tests for the `brix` executable (ADR-0010, ADR-0026, ADR-0030).
//!
//! Tests the built CLI binary via `env!("CARGO_BIN_EXE_brix")` across all 12 contract areas:
//! 1. help/version behavior
//! 2. exact JSON field/schema/tag shapes
//! 3. byte-identical repeated run output (plain text and --json)
//! 4. shipping expected facts, statuses, and Ship @Derived decision
//! 5. quiescence and Unknown rejection/exit 1
//! 6. why/whynot explained winner, rejected guard, overshadowed, candidate not found
//! 7. audit refusal without --force, overwrite with --force, and temp cleanup on failure
//! 8. cross-process audit then verify using emitted external program pin
//! 9. pin tampering, source tampering, and bundle tampering verification failures
//! 10. preserved L3 v1 verify compatibility
//! 11. embedded brix.soc package precedence over external directories
//! 12. explicit --package-path requirement and absence of ambient lookup

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use brix_canon::Canonical;

fn repo_root() -> PathBuf {
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    manifest_dir
        .parent()
        .expect("crates parent")
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn brix_bin() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_brix")
        .or_else(|| std::env::var_os("NEXTEST_BIN_EXE_brix"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_brix")))
}

fn brix() -> Command {
    let mut cmd = Command::new(brix_bin());
    cmd.current_dir(repo_root());
    cmd
}

fn run_cmd(mut cmd: Command) -> (i32, String, String) {
    let output: Output = cmd.output().expect("failed to execute brix binary");
    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (code, stdout, stderr)
}

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "brix_test_{}_{}_{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("failed to create temp test directory");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

// ---------------------------------------------------------------------------
// 1. Help and Version Behavior
// ---------------------------------------------------------------------------

#[test]
fn test_01_help_and_version_behavior() {
    // Global --help exits 0 and prints usage and subcommands
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("--help");
        c
    });
    assert_eq!(code, 0);
    assert!(stdout.contains("Usage: brix"));
    assert!(stdout.contains("check"));
    assert!(stdout.contains("run"));
    assert!(stdout.contains("audit"));
    assert!(stdout.contains("verify"));
    assert!(stdout.contains("why"));
    assert!(stdout.contains("whynot"));
    assert!(stdout.contains("--help"));
    assert!(stdout.contains("--version"));

    // Global --version exits 0 and prints version
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("--version");
        c
    });
    assert_eq!(code, 0);
    assert!(stdout.contains("brix"));

    // Subcommand aliases and short aliases rejected with exit code 2
    for rejected in ["help", "version", "-h", "-V", "-v"] {
        let (code, _, stderr) = run_cmd({
            let mut c = brix();
            c.arg(rejected);
            c
        });
        assert_eq!(
            code, 2,
            "argument '{rejected}' should exit 2 as usage error"
        );
        assert!(
            stderr.contains("usage error") || stderr.contains("unknown"),
            "stderr for '{rejected}': {stderr}"
        );
    }

    // Subcommand-level --help and --version rejected with exit code 2
    for sub in ["check", "run", "audit", "verify", "why", "whynot"] {
        let (code, _, _) = run_cmd({
            let mut c = brix();
            c.arg(sub).arg("--help");
            c
        });
        assert_eq!(code, 2, "{sub} --help should exit 2");

        let (code, _, _) = run_cmd({
            let mut c = brix();
            c.arg(sub).arg("--version");
            c
        });
        assert_eq!(code, 2, "{sub} --version should exit 2");
    }
}

// ---------------------------------------------------------------------------
// 2. Exact JSON Field, Schema, and Tag Shapes
// ---------------------------------------------------------------------------

#[test]
fn test_02_exact_json_field_schema_and_tag_shapes() {
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("run").arg("--json").arg("examples/shipping.brix");
        c
    });
    assert_eq!(code, 0, "run failed: {stderr}");

    // Schema must be the very first field in JSON serialization
    let schema_idx = stdout
        .find(r#""schema": "brix.cli.result@1""#)
        .expect("schema marker missing");
    let command_idx = stdout.find(r#""command""#).expect("command field missing");
    assert!(
        schema_idx < command_idx,
        "schema must appear before command in JSON"
    );

    let val: serde_json::Value =
        serde_json::from_str(&stdout).expect("valid JSON must be returned");
    let obj = val.as_object().expect("top-level result must be an object");

    // Must have all 12 required fields
    let expected_keys = [
        "schema",
        "command",
        "ok",
        "profile",
        "program",
        "context",
        "status",
        "facts",
        "candidates",
        "decision",
        "artifacts",
        "diagnostics",
    ];
    for key in expected_keys {
        assert!(obj.contains_key(key), "missing required JSON field: {key}");
    }
    assert_eq!(obj.len(), 12, "must contain exactly 12 top-level fields");

    assert_eq!(obj["schema"], "brix.cli.result@1");
    assert_eq!(obj["command"], "run");
    assert_eq!(obj["ok"], true);
    assert_eq!(obj["profile"], "brix.l3.finite-decision@1");
    assert!(obj["program"].is_string());
    assert!(obj["context"].is_string());
    assert_eq!(obj["status"], "selected");

    // Validate facts: values use single discriminator "type"
    let facts = obj["facts"].as_array().expect("facts must be an array");
    assert_eq!(facts.len(), 2);
    for fact in facts {
        let f = fact.as_object().unwrap();
        assert!(f.contains_key("name"));
        assert!(f.contains_key("value"));
        assert!(f["ordinal"].is_string()); // Semantic integer as string
        assert_eq!(f["grade"], "Derived");

        let v = f["value"].as_object().unwrap();
        assert_eq!(v["type"], "int");
        assert!(v["value"].is_string()); // Decimal string value
        assert!(!v.contains_key("tag"));
        assert!(!v.contains_key("type_name"));
    }

    // Validate candidates: statuses strictly from allowed set
    let candidates = obj["candidates"]
        .as_array()
        .expect("candidates must be an array");
    assert_eq!(candidates.len(), 3);
    for cand in candidates {
        let c = cand.as_object().unwrap();
        assert!(c["priority"].is_string()); // Semantic integer as string
        let status = c["status"].as_str().unwrap();
        assert!(
            [
                "selected",
                "admitted-not-selected",
                "rejected-guard-false",
                "rejected"
            ]
            .contains(&status),
            "invalid candidate status: {status}"
        );
        let reason = c["reason"].as_object().unwrap();
        assert!(reason.contains_key("code"));
        assert!(reason.contains_key("detail"));
    }

    // Validate decision: single discriminator "type": "sum"
    let decision = obj["decision"]
        .as_object()
        .expect("decision must be an object");
    assert_eq!(decision["candidate"], "ship");
    assert_eq!(decision["priority"], "10"); // Semantic integer as string
    assert_eq!(decision["grade"], "Derived");
    let dec_val = decision["value"].as_object().unwrap();
    assert_eq!(dec_val["type"], "sum");
    assert_eq!(dec_val["nominal"], "Decision");
    assert_eq!(dec_val["variant"], "Ship");
    assert_eq!(dec_val["args"].as_array().unwrap().len(), 0);
    assert!(!dec_val.contains_key("nominal_sum"));

    // Also test failure JSON: must also have 12 fields and schema first
    let (code_err, stdout_err, _) = run_cmd({
        let mut c = brix();
        c.arg("check")
            .arg("--json")
            .arg("nonexistent_test_file.brix");
        c
    });
    assert_eq!(code_err, 2);
    let val_err: serde_json::Value = serde_json::from_str(&stdout_err).unwrap();
    let obj_err = val_err.as_object().unwrap();
    assert_eq!(obj_err["schema"], "brix.cli.result@1");
    assert_eq!(obj_err["command"], "check");
    assert_eq!(obj_err["ok"], false);
    assert_eq!(obj_err["status"], "io-error");
    assert_eq!(obj_err.len(), 12);
}

// ---------------------------------------------------------------------------
// 3. Byte-Identical Repeated Run Output
// ---------------------------------------------------------------------------

#[test]
fn test_03_byte_identical_repeated_run_output() {
    // Plain text repeated run
    let (code1, stdout1, stderr1) = run_cmd({
        let mut c = brix();
        c.arg("run").arg("examples/shipping.brix");
        c
    });
    let (code2, stdout2, stderr2) = run_cmd({
        let mut c = brix();
        c.arg("run").arg("examples/shipping.brix");
        c
    });
    assert_eq!(code1, 0, "run 1 failed: {stderr1}");
    assert_eq!(code2, 0, "run 2 failed: {stderr2}");
    assert_eq!(stdout1, stdout2, "plain text run must be byte-identical");
    assert_eq!(stderr1, stderr2);

    // JSON repeated run
    let (code_j1, stdout_j1, stderr_j1) = run_cmd({
        let mut c = brix();
        c.arg("run").arg("--json").arg("examples/shipping.brix");
        c
    });
    let (code_j2, stdout_j2, stderr_j2) = run_cmd({
        let mut c = brix();
        c.arg("run").arg("--json").arg("examples/shipping.brix");
        c
    });
    assert_eq!(code_j1, 0, "json run 1 failed: {stderr_j1}");
    assert_eq!(code_j2, 0, "json run 2 failed: {stderr_j2}");
    assert_eq!(stdout_j1, stdout_j2, "JSON run must be byte-identical");
    assert_eq!(stderr_j1, stderr_j2);
}

// ---------------------------------------------------------------------------
// 4. Shipping Expected Facts, Statuses, and Ship @Derived Decision
// ---------------------------------------------------------------------------

#[test]
fn test_04_shipping_expected_facts_statuses_and_decision() {
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("run").arg("examples/shipping.brix");
        c
    });
    assert_eq!(code, 0, "run failed: {stderr}");

    // Facts
    assert!(stdout.contains("facts:"));
    assert!(stdout.contains("  stock: 12 @Derived"));
    assert!(stdout.contains("  threshold: 10 @Derived"));

    // Candidates
    assert!(stdout.contains("candidates:"));
    assert!(stdout.contains(
        "  expedite: rejected-guard-false (priority 5) — guard condition evaluated to false"
    ));
    assert!(stdout.contains("  ship: selected (priority 10) — selected: minimal calendar key"));
    assert!(stdout.contains("  hold: admitted-not-selected (priority 100) — admitted but overshadowed by candidate 'ship'"));

    // Decision and Status
    assert!(stdout.contains("decision: ship = Ship @Derived"));
    assert!(stdout.contains("status: selected"));

    // IDs
    assert!(stdout.contains("program: "));
    assert!(stdout.contains("context: "));

    // Disciplinary rule: Never prints Proven or Refuted
    assert!(
        !stdout.contains("Proven"),
        "Disciplinary rule: must not print Proven"
    );
    assert!(
        !stdout.contains("Refuted"),
        "Disciplinary rule: must not print Refuted"
    );
}

// ---------------------------------------------------------------------------
// 5. Quiescence and Unknown Rejection / Exit 1
// ---------------------------------------------------------------------------

#[test]
fn test_05_quiescence_and_unknown_rejection() {
    let temp = TempDirGuard::new("quiescence_unknown");

    // A. Quiescent program: base is 2, guard checks base == 1 -> no candidate admitted
    let quiescent_src = r#"
config Decision = Hold
rule base() = 2
propose hold(base) priority 10 when base == 1 = Hold
commit pick from (hold)
show pick
"#;
    let quiescent_path = temp.path().join("quiescent.brix");
    fs::write(&quiescent_path, quiescent_src).unwrap();

    // Plain text run: quiescent
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("run").arg(&quiescent_path);
        c
    });
    assert_eq!(code, 0, "quiescent program must exit 0; stderr: {stderr}");
    assert!(stdout.contains("decision: none (quiescent)"));
    assert!(stdout.contains("status: quiescent"));

    // JSON run: quiescent
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("run").arg("--json").arg(&quiescent_path);
        c
    });
    assert_eq!(code, 0);
    let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(val["status"], "quiescent");
    assert_eq!(val["ok"], true);
    assert!(val["decision"].is_null());

    // B. Unknown program: runtime type fault in guard
    let unknown_src = r#"
config Val = Num
rule number() = 42
propose p(number) priority 1 when number = Num
commit pick from (p)
show pick
"#;
    let unknown_path = temp.path().join("unknown.brix");
    fs::write(&unknown_path, unknown_src).unwrap();

    // Plain text run: unknown -> exits 1
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("run").arg(&unknown_path);
        c
    });
    assert_eq!(code, 1, "unknown program must exit 1");
    assert!(stdout.contains("status: unknown"));

    // JSON run: unknown -> exits 1, diagnostics has code and detail
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("run").arg("--json").arg(&unknown_path);
        c
    });
    assert_eq!(code, 1);
    let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(val["status"], "unknown");
    assert_eq!(val["ok"], false);
    let diags = val["diagnostics"].as_array().unwrap();
    assert!(!diags.is_empty());
    assert!(diags[0].as_str().unwrap().contains("type-fault"));

    // Check command: preflight must detect Unknown and exit 1
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("check").arg("--json").arg(&unknown_path);
        c
    });
    assert_eq!(code, 1, "check preflight must reject unknown with exit 1");
    let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(val["status"], "unknown");
    assert_eq!(val["ok"], false);
}

// ---------------------------------------------------------------------------
// 6. Why and Whynot Explanations
// ---------------------------------------------------------------------------

#[test]
fn test_06_why_and_whynot_explanations() {
    let fixture = "examples/shipping.brix";

    // 1. Explained winner: ship
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("why").arg(fixture).arg("--candidate").arg("ship");
        c
    });
    assert_eq!(code, 0, "why ship failed: {stderr}");
    assert!(stdout.contains("ship: selected — admitted with minimal calendar key"));

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("whynot").arg(fixture).arg("--candidate").arg("ship");
        c
    });
    assert_eq!(code, 0, "whynot ship failed: {stderr}");
    assert!(stdout.contains("ship: actually-selected — candidate was admitted and selected"));

    // 2. Rejected guard: expedite
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("why").arg(fixture).arg("--candidate").arg("expedite");
        c
    });
    assert_eq!(code, 0, "why expedite failed: {stderr}");
    assert!(stdout.contains("expedite: not-admitted — rejected (rejected guard-false)"));

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("whynot")
            .arg(fixture)
            .arg("--candidate")
            .arg("expedite");
        c
    });
    assert_eq!(code, 0, "whynot expedite failed: {stderr}");
    assert!(stdout.contains("expedite: rejected — rejected guard-false"));

    // 3. Overshadowed: hold
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("why").arg(fixture).arg("--candidate").arg("hold");
        c
    });
    assert_eq!(code, 0, "why hold failed: {stderr}");
    assert!(
        stdout.contains("hold: admitted-not-selected — overshadowed by selected candidate 'ship'")
    );

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("whynot").arg(fixture).arg("--candidate").arg("hold");
        c
    });
    assert_eq!(code, 0, "whynot hold failed: {stderr}");
    assert!(stdout.contains(
        "hold: overshadowed — admitted but lost selection to higher-priority candidate 'ship'"
    ));

    // 4. Candidate not found in pool
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("why")
            .arg("--json")
            .arg(fixture)
            .arg("--candidate")
            .arg("nonexistent_cand");
        c
    });
    assert_eq!(code, 1);
    let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(val["status"], "candidate-not-found");
    assert_eq!(val["ok"], false);

    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("whynot")
            .arg("--json")
            .arg(fixture)
            .arg("--candidate")
            .arg("nonexistent_cand");
        c
    });
    assert_eq!(code, 1);
    let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(val["status"], "candidate-not-found");
    assert_eq!(val["ok"], false);
}

// ---------------------------------------------------------------------------
// 7. Audit Refusal Without --force, Overwrite With --force, and Failure Cleanup
// ---------------------------------------------------------------------------

#[test]
fn test_07_audit_refusal_force_and_cleanup() {
    let temp = TempDirGuard::new("audit_refusal");
    let bundle_path = temp.path().join("test_bundle.bin");

    // 1. Initial audit succeeds
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("audit")
            .arg("examples/shipping.brix")
            .arg("--bundle")
            .arg(&bundle_path);
        c
    });
    assert_eq!(code, 0, "audit failed: {stderr}");
    assert!(bundle_path.exists());
    assert!(stdout.contains("status: audited"));

    // 2. Audit without --force is refused with exit code 2
    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("audit")
            .arg("examples/shipping.brix")
            .arg("--bundle")
            .arg(&bundle_path);
        c
    });
    assert_eq!(code, 2, "audit without --force must refuse existing file");
    assert!(stderr.contains("already exists (use --force to overwrite)"));

    // 3. Audit with --force overwrites successfully
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("audit")
            .arg("examples/shipping.brix")
            .arg("--bundle")
            .arg(&bundle_path)
            .arg("--force");
        c
    });
    assert_eq!(code, 0, "audit with --force must succeed; stderr: {stderr}");
    assert!(stdout.contains("status: audited"));

    // 4. Failure cleanup: write failure or Unknown halts without leaving temp or destination files
    let unknown_src = r#"
config Val = Num
rule number() = 42
propose p(number) priority 1 when number = Num
commit pick from (p)
show pick
"#;
    let unknown_brix = temp.path().join("unknown.brix");
    fs::write(&unknown_brix, unknown_src).unwrap();

    let failed_bundle = temp.path().join("failed_bundle.bin");
    let (code, _, _) = run_cmd({
        let mut c = brix();
        c.arg("audit")
            .arg(&unknown_brix)
            .arg("--bundle")
            .arg(&failed_bundle);
        c
    });
    assert_eq!(code, 1, "audit on unknown program must exit 1");
    assert!(!failed_bundle.exists(), "no destination file on failure");

    // Ensure no temp files matching .tmp_bundle_* exist in directory
    for entry in fs::read_dir(temp.path()).unwrap() {
        let entry = entry.unwrap();
        let fname = entry.file_name().to_string_lossy().to_string();
        assert!(
            !fname.starts_with(".tmp_bundle_"),
            "found orphaned temp file: {fname}"
        );
    }
}

// ---------------------------------------------------------------------------
// 8. Cross-Process Audit Then Verify Using Emitted External Program Pin
// ---------------------------------------------------------------------------

#[test]
fn test_08_cross_process_audit_then_verify() {
    let temp = TempDirGuard::new("cross_process");
    let bundle_path = temp.path().join("shipping.bundle");

    // 1. Audit to produce bundle and JSON metadata
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("audit")
            .arg("--json")
            .arg("examples/shipping.brix")
            .arg("--bundle")
            .arg(&bundle_path);
        c
    });
    assert_eq!(code, 0, "audit failed: {stderr}");
    let audit_json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(audit_json["status"], "audited");
    let program_pin = audit_json["program"].as_str().unwrap();
    assert_eq!(program_pin.len(), 64);

    let artifacts = audit_json["artifacts"].as_array().unwrap();
    assert_eq!(artifacts.len(), 1);
    let bundle_artifact = &artifacts[0];
    assert_eq!(bundle_artifact["kind"], "audit-bundle");
    assert!(bundle_artifact["bundle_id"].is_string());
    assert!(bundle_artifact["final_chain_digest"].is_string());
    assert!(bundle_artifact["count"].is_string());

    // 2. Cross-process verify with the emitted external program pin
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("verify")
            .arg("--json")
            .arg("--expect-program")
            .arg(program_pin)
            .arg("examples/shipping.brix")
            .arg(&bundle_path);
        c
    });
    assert_eq!(code, 0, "verify failed: {stderr}");
    let verify_json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(verify_json["status"], "audit-bundle-verified");
    assert_eq!(verify_json["ok"], true);
    assert_eq!(verify_json["program"], program_pin);
    assert_eq!(verify_json["profile"], "brix.l3.finite-decision@1");

    let v_artifacts = verify_json["artifacts"].as_array().unwrap();
    assert_eq!(v_artifacts.len(), 1);
    assert_eq!(v_artifacts[0]["kind"], "audit-bundle");
    assert_eq!(v_artifacts[0]["bundle_id"], bundle_artifact["bundle_id"]);
    assert_eq!(
        v_artifacts[0]["final_chain_digest"],
        bundle_artifact["final_chain_digest"]
    );
}

// ---------------------------------------------------------------------------
// 9. Tampering Verification Failures (Pin, Source, Bundle)
// ---------------------------------------------------------------------------

#[test]
fn test_09_tampering_verification_failures() {
    let temp = TempDirGuard::new("tampering");
    let bundle_path = temp.path().join("shipping.bundle");

    // Create valid audit bundle
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("audit")
            .arg("--json")
            .arg("examples/shipping.brix")
            .arg("--bundle")
            .arg(&bundle_path);
        c
    });
    assert_eq!(code, 0, "audit failed: {stderr}");
    let audit_json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let program_pin = audit_json["program"].as_str().unwrap();

    // A. Pin tampering: pass incorrect expected program
    let bogus_pin = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("verify")
            .arg("--expect-program")
            .arg(bogus_pin)
            .arg("examples/shipping.brix")
            .arg(&bundle_path);
        c
    });
    assert_eq!(code, 1, "tampered pin must fail verification with exit 1");
    assert!(stderr.contains("program identity mismatch"));

    // B. Source tampering: modify source code
    let tampered_src = fs::read_to_string(repo_root().join("examples/shipping.brix"))
        .unwrap()
        .replace("rule threshold() = 10", "rule threshold() = 11");
    let tampered_src_path = temp.path().join("tampered_shipping.brix");
    fs::write(&tampered_src_path, tampered_src).unwrap();

    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("verify")
            .arg("--expect-program")
            .arg(program_pin)
            .arg(&tampered_src_path)
            .arg(&bundle_path);
        c
    });
    assert_eq!(
        code, 1,
        "tampered source must fail verification with exit 1"
    );
    assert!(
        stderr.contains("program identity mismatch")
            || stderr.contains("context identity mismatch")
    );

    // C. Bundle tampering: modify bundle bytes
    let mut bundle_bytes = fs::read(&bundle_path).unwrap();
    let last = bundle_bytes.len() - 1;
    bundle_bytes[last] ^= 0xff; // Flip bits in last byte
    let tampered_bundle_path = temp.path().join("tampered.bundle");
    fs::write(&tampered_bundle_path, bundle_bytes).unwrap();

    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("verify")
            .arg("--expect-program")
            .arg(program_pin)
            .arg("examples/shipping.brix")
            .arg(&tampered_bundle_path);
        c
    });
    assert_eq!(
        code, 1,
        "tampered bundle must fail verification with exit 1"
    );
    assert!(stderr.contains("bundle verification failed") || stderr.contains("rejected"));
}

// ---------------------------------------------------------------------------
// 10. Preserved L3 v1 Verify Compatibility
// ---------------------------------------------------------------------------

#[test]
fn test_10_preserved_l3_v1_verify_compatibility() {
    let temp = TempDirGuard::new("l3_v1_compat");

    let l3_src = "rule a() = 1\nrule b() = 2\n";
    let l3_file = temp.path().join("l3_rules.brix");
    fs::write(&l3_file, l3_src).unwrap();

    // Produce an L3 v1 audit bundle using the lower crate
    let module = brix_syntax::parse(l3_src).unwrap();
    let plan = brix_lower::lower_l3_plan(
        &module,
        brix_lower::L3_PROFILE_MARKER_V1,
        &brix_lower::PlanLimitsV1::generous(),
    )
    .unwrap();
    let expected_prog = brix_lower::program_id(&plan);
    let expected_prog_hex = expected_prog.0.to_hex();

    let report = brix_lower::run_l3_plan(
        &plan,
        brix_lower::L3AdmChoice::Compiled,
        soc_core::saturate::SaturationBudget::uniform(1_000),
    );
    let bundle = brix_lower::produce_l3_audit_input_bundle_v1(&report, &report.run).unwrap();
    let bundle_bytes = bundle.canon_bytes();

    let bundle_file = temp.path().join("l3_audit.bundle");
    fs::write(&bundle_file, bundle_bytes).unwrap();

    // Verify via CLI using --profile l3-v1
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("verify")
            .arg("--json")
            .arg("--expect-program")
            .arg(&expected_prog_hex)
            .arg(&l3_file)
            .arg(&bundle_file)
            .arg("--profile")
            .arg("l3-v1");
        c
    });
    assert_eq!(
        code, 0,
        "L3 v1 bundle verification must succeed; stderr: {stderr}"
    );
    let val: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(val["status"], "audit-bundle-verified");
    assert_eq!(val["ok"], true);
    assert_eq!(val["program"], expected_prog_hex);
    // Preserved canonical profile marker must be reported, not the CLI alias "l3-v1"
    assert_eq!(val["profile"], "brix.l3.rule-agenda-saturated@1");

    let artifacts = val["artifacts"].as_array().unwrap();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0]["kind"], "audit-bundle");
    assert_eq!(artifacts[0]["count"], "2");
}

// ---------------------------------------------------------------------------
// 11. Embedded brix.soc Package Precedence Over External Directories
// ---------------------------------------------------------------------------

#[test]
fn test_11_embedded_brix_soc_precedence() {
    let temp = TempDirGuard::new("soc_precedence");

    // Create an external package directory for brix.soc with completely broken syntax
    let external_soc_src = temp.path().join("brix.soc/src");
    fs::create_dir_all(&external_soc_src).unwrap();
    fs::write(
        external_soc_src.join("soc.brix"),
        "THIS_IS_INVALID_SYNTAX_THAT_CANNOT_PARSE !@#$%^\n",
    )
    .unwrap();

    // Create a source file that uses brix.soc
    let test_src = r#"
use brix.soc

let local_binding = 42
"#;
    let test_file = temp.path().join("test_soc_import.brix");
    fs::write(&test_file, test_src).unwrap();

    // Check with explicit --package-path pointing to the directory containing broken brix.soc
    // Because embedded brix.soc takes precedence, it will ignore the broken disk file and succeed!
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("check")
            .arg(&test_file)
            .arg("--package-path")
            .arg(temp.path());
        c
    });
    assert_eq!(
        code, 0,
        "check should succeed with embedded precedence; stderr: {stderr}"
    );
    assert!(stdout.contains("accepted") || stdout.contains("local_binding"));
}

// ---------------------------------------------------------------------------
// 12. Explicit --package-path Requirement and Absence of Ambient Lookup
// ---------------------------------------------------------------------------

#[test]
fn test_12_explicit_package_path_and_no_ambient_lookup() {
    let temp = TempDirGuard::new("package_path_req");

    // Create standard package structure: <pkg_root>/custom.pkg/src/pkg.brix
    let pkg_root = temp.path().join("packages");
    let pkg_src = pkg_root.join("custom.pkg/src");
    fs::create_dir_all(&pkg_src).unwrap();
    fs::write(
        pkg_src.join("pkg.brix"),
        "config CustomConfig = Alpha | Beta\n",
    )
    .unwrap();

    // Create consumer file importing custom.pkg
    let consumer_src = r#"
use custom.pkg

let consumer_value = 100
"#;
    let consumer_file = temp.path().join("consumer.brix");
    fs::write(&consumer_file, consumer_src).unwrap();

    // A. Without --package-path: MUST FAIL (no ambient lookup)
    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("check").arg(&consumer_file);
        c
    });
    assert_ne!(
        code, 0,
        "import must fail when --package-path is omitted (no ambient lookup)"
    );
    assert!(stderr.contains("rejected") || stderr.contains("import"));

    // B. With --package-path <pkg_root>: MUST SUCCEED
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("check")
            .arg(&consumer_file)
            .arg("--package-path")
            .arg(&pkg_root);
        c
    });
    assert_eq!(
        code, 0,
        "import must succeed with explicit --package-path; stderr: {stderr}"
    );
    assert!(stdout.contains("accepted") || stdout.contains("consumer_value"));

    // C. Flat layout rejection: <flat_root>/custom.flat/flat.brix (no src/ directory)
    let flat_root = temp.path().join("flat_packages");
    let flat_pkg = flat_root.join("custom.flat");
    fs::create_dir_all(&flat_pkg).unwrap();
    fs::write(flat_pkg.join("flat.brix"), "config FlatConfig = Flat\n").unwrap();

    let flat_consumer_src = r#"
use custom.flat

let flat_consumer_val = 1
"#;
    let flat_consumer = temp.path().join("flat_consumer.brix");
    fs::write(&flat_consumer, flat_consumer_src).unwrap();

    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("check")
            .arg(&flat_consumer)
            .arg("--package-path")
            .arg(&flat_root);
        c
    });
    assert_ne!(
        code, 0,
        "flat package layout must be rejected even with --package-path"
    );
    assert!(stderr.contains("rejected") || stderr.contains("import"));
}

// ---------------------------------------------------------------------------
// 13. External Inputs: Check Contract and Preflight
// ---------------------------------------------------------------------------

#[test]
fn test_13_external_input_check_contract_and_preflight() {
    let temp = TempDirGuard::new("ext_input_check");
    let brix_file = temp.path().join("model.brix");
    let brix_src = r#"
input limit: Int
input enabled: Bool

config Arrangement = A | B

rule base() = limit
rule is_enabled() = enabled

propose opt_a(base) priority 10 when base == 100 = A
propose opt_b(is_enabled) priority 20 when is_enabled = B

commit pick from (opt_a, opt_b)
"#;
    fs::write(&brix_file, brix_src).unwrap();

    let input_valid = temp.path().join("input_valid.json");
    let input_valid_json = r#"{
  "schema": "brix.input@1",
  "values": {
    "limit": { "type": "int", "value": "100" },
    "enabled": { "type": "bool", "value": true }
  }
}"#;
    fs::write(&input_valid, input_valid_json).unwrap();

    let input_incomplete = temp.path().join("input_incomplete.json");
    let input_incomplete_json = r#"{
  "schema": "brix.input@1",
  "values": {
    "limit": { "type": "int", "value": "100" }
  }
}"#;
    fs::write(&input_incomplete, input_incomplete_json).unwrap();

    // 1. check without --input: declaration-only contract check
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("check").arg(&brix_file);
        c
    });
    assert_eq!(
        code, 0,
        "check declaration-only must exit 0; stderr: {stderr}"
    );
    assert!(stdout.contains("status: checked-input-contract"));
    assert!(stdout.contains("program: "));
    assert!(
        !stdout.contains("context: "),
        "declaration check must not emit context identity"
    );

    // 2. check without --input in JSON mode
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("check").arg(&brix_file).arg("--json");
        c
    });
    assert_eq!(code, 0, "check --json must exit 0; stderr: {stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(v["schema"], "brix.cli.result@1");
    assert_eq!(v["ok"], true);
    assert_eq!(v["status"], "checked-input-contract");
    assert!(v["program"].is_string());
    assert!(v["context"].is_null());
    assert!(!v.as_object().unwrap().contains_key("input_snapshot"));
    assert!(!v.as_object().unwrap().contains_key("inputs"));
    assert_eq!(v.as_object().unwrap().len(), 12);

    // 3. check with complete --input: preflight deliberation passes
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("check")
            .arg(&brix_file)
            .arg("--input")
            .arg(&input_valid);
        c
    });
    assert_eq!(
        code, 0,
        "check with valid input must exit 0; stderr: {stderr}"
    );
    assert!(stdout.contains("status: selected") || stdout.contains("decision: opt_a"));
    assert!(stdout.contains("inputs:"));
    assert!(stdout.contains("@Derived"));
    assert!(stdout.contains("input-snapshot:"));

    // 4. check with complete --input in JSON mode
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("check")
            .arg(&brix_file)
            .arg("--input")
            .arg(&input_valid)
            .arg("--json");
        c
    });
    assert_eq!(
        code, 0,
        "check with valid input --json must exit 0; stderr: {stderr}"
    );
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(v["schema"], "brix.cli.result@1");
    assert_eq!(v["ok"], true);
    assert_eq!(v["status"], "accepted");
    assert!(v["input_snapshot"].is_string());
    assert!(v["inputs"].is_array());
    assert_eq!(v.as_object().unwrap().len(), 14);

    // 5. check with incomplete --input: preflight build fails (missing input)
    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("check")
            .arg(&brix_file)
            .arg("--input")
            .arg(&input_incomplete);
        c
    });
    assert_eq!(
        code, 1,
        "check with incomplete input must exit 1 (rejected)"
    );
    assert!(stderr.contains("rejected") || stderr.contains("missing"));
}

// ---------------------------------------------------------------------------
// 14. External Inputs: Run Rejections and Success Modes
// ---------------------------------------------------------------------------

#[test]
fn test_14_external_input_run_rejections_and_success() {
    let temp = TempDirGuard::new("ext_input_run");
    let brix_file = temp.path().join("full_model.brix");
    let brix_src = r#"
input limit: Int
input flag: Bool
input label: Str

config Output = Res(Int)

rule base() = limit
rule is_flag() = flag
rule tag() = label

propose main(base) priority 10 when base == 42 = Res(base)

commit pick from (main)
"#;
    fs::write(&brix_file, brix_src).unwrap();

    let valid_json = r#"{
  "schema": "brix.input@1",
  "values": {
    "limit": { "type": "int", "value": "42" },
    "flag": { "type": "bool", "value": true },
    "label": { "type": "string", "value": "production" }
  }
}"#;
    let input_valid = temp.path().join("valid.json");
    fs::write(&input_valid, valid_json).unwrap();

    // 1. Missing inputs on run (no --input provided)
    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("run").arg(&brix_file);
        c
    });
    assert_eq!(code, 1, "run with missing inputs must exit 1");
    assert!(stderr.contains("rejected"));

    // 2. Extra undeclared input
    let extra_json = r#"{
  "schema": "brix.input@1",
  "values": {
    "limit": { "type": "int", "value": "42" },
    "flag": { "type": "bool", "value": true },
    "label": { "type": "string", "value": "production" },
    "extra": { "type": "int", "value": "99" }
  }
}"#;
    let input_extra = temp.path().join("extra.json");
    fs::write(&input_extra, extra_json).unwrap();
    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(&brix_file)
            .arg("--input")
            .arg(&input_extra);
        c
    });
    assert_eq!(code, 1, "run with extra input must exit 1");
    assert!(stderr.contains("rejected"));

    // 3. Type mismatch
    let mismatch_json = r#"{
  "schema": "brix.input@1",
  "values": {
    "limit": { "type": "string", "value": "42" },
    "flag": { "type": "bool", "value": true },
    "label": { "type": "string", "value": "production" }
  }
}"#;
    let input_mismatch = temp.path().join("mismatch.json");
    fs::write(&input_mismatch, mismatch_json).unwrap();
    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(&brix_file)
            .arg("--input")
            .arg(&input_mismatch);
        c
    });
    assert_eq!(code, 1, "run with type mismatch must exit 1");
    assert!(stderr.contains("rejected"));

    // 4. Duplicate key in single shard
    let dup_json = r#"{
  "schema": "brix.input@1",
  "values": {
    "limit": { "type": "int", "value": "42" },
    "limit": { "type": "int", "value": "42" },
    "flag": { "type": "bool", "value": true },
    "label": { "type": "string", "value": "production" }
  }
}"#;
    let input_dup = temp.path().join("dup.json");
    fs::write(&input_dup, dup_json).unwrap();
    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("run").arg(&brix_file).arg("--input").arg(&input_dup);
        c
    });
    assert_eq!(
        code, 1,
        "run with duplicate key in single shard must exit 1"
    );
    assert!(stderr.contains("rejected"));

    // 5. Successful execution in plain text mode
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(&brix_file)
            .arg("--input")
            .arg(&input_valid);
        c
    });
    assert_eq!(
        code, 0,
        "run with valid inputs must exit 0; stderr: {stderr}"
    );
    assert!(stdout.contains("inputs:"));
    assert!(stdout.contains("limit: 42 @Derived"));
    assert!(stdout.contains("flag: true @Derived"));
    assert!(stdout.contains("label: \"production\" @Derived"));
    assert!(stdout.contains("decision: main = Res(42) @Derived"));
    assert!(stdout.contains("status: selected"));
    assert!(stdout.contains("input-snapshot:"));
    assert!(
        !stdout.contains("Proven"),
        "inputs must never be labeled Proven"
    );
    assert!(
        !stdout.contains("Audited"),
        "inputs must never be labeled Audited"
    );

    // 6. Successful execution in JSON mode
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(&brix_file)
            .arg("--input")
            .arg(&input_valid)
            .arg("--json");
        c
    });
    assert_eq!(code, 0, "run --json must exit 0; stderr: {stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(v["schema"], "brix.cli.result@1");
    assert_eq!(v["ok"], true);
    assert_eq!(v["status"], "selected");
    assert_eq!(
        v.as_object().unwrap().len(),
        14,
        "must contain exactly 14 fields"
    );
    assert!(v["input_snapshot"].is_string());
    let inps = v["inputs"].as_array().expect("inputs is array");
    assert_eq!(inps.len(), 3);
    for inp in inps {
        assert_eq!(inp["grade"], "Derived");
        assert!(inp.get("name").is_some());
        assert!(inp.get("ordinal").is_some());
        assert!(inp.get("value").is_some());
    }
}

// ---------------------------------------------------------------------------
// 15. External Inputs: Shard Order Determinism and Identity Invariants
// ---------------------------------------------------------------------------

#[test]
fn test_15_external_input_determinism_and_identity() {
    let temp = TempDirGuard::new("ext_input_determinism");
    let brix_file = temp.path().join("shards_model.brix");
    let brix_src = r#"
input limit: Int
input enabled: Bool
input label: Str

config Res = Win

rule base() = limit
rule is_enabled() = enabled

propose p(base) priority 1 when base > 0 = Win

commit pick from (p)
"#;
    fs::write(&brix_file, brix_src).unwrap();

    let shard_a = temp.path().join("shard_a.json");
    let shard_a_json = r#"{
  "schema": "brix.input@1",
  "values": {
    "limit": { "type": "int", "value": "100" },
    "enabled": { "type": "bool", "value": true }
  }
}"#;
    fs::write(&shard_a, shard_a_json).unwrap();

    let shard_b = temp.path().join("shard_b.json");
    let shard_b_json = r#"{
  "schema": "brix.input@1",
  "values": {
    "label": { "type": "string", "value": "region-alpha" }
  }
}"#;
    fs::write(&shard_b, shard_b_json).unwrap();

    // 1. Shard order [A, B]
    let (code1, stdout1, _) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(&brix_file)
            .arg("--input")
            .arg(&shard_a)
            .arg("--input")
            .arg(&shard_b)
            .arg("--json");
        c
    });
    assert_eq!(code1, 0);

    // 2. Shard order [B, A]
    let (code2, stdout2, _) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(&brix_file)
            .arg("--input")
            .arg(&shard_b)
            .arg("--input")
            .arg(&shard_a)
            .arg("--json");
        c
    });
    assert_eq!(code2, 0);

    let v1: serde_json::Value = serde_json::from_str(&stdout1).unwrap();
    let v2: serde_json::Value = serde_json::from_str(&stdout2).unwrap();

    assert_eq!(
        v1["input_snapshot"], v2["input_snapshot"],
        "input_snapshot must be shard-order invariant"
    );
    assert_eq!(
        v1["context"], v2["context"],
        "context must be shard-order invariant"
    );
    assert_eq!(
        v1["program"], v2["program"],
        "program must be shard-order invariant"
    );

    // 3. Duplicate key across disjoint shards rejected
    let shard_overlap = temp.path().join("shard_overlap.json");
    let shard_overlap_json = r#"{
  "schema": "brix.input@1",
  "values": {
    "limit": { "type": "int", "value": "200" }
  }
}"#;
    fs::write(&shard_overlap, shard_overlap_json).unwrap();
    let (code_overlap, _, stderr_overlap) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(&brix_file)
            .arg("--input")
            .arg(&shard_a)
            .arg("--input")
            .arg(&shard_overlap);
        c
    });
    assert_eq!(
        code_overlap, 1,
        "duplicate input across disjoint shards must exit 1"
    );
    assert!(stderr_overlap.contains("rejected") || stderr_overlap.contains("duplicate"));

    // 4. Varying input value preserves ProgramId while ContextId changes
    let shard_a_varied = temp.path().join("shard_a_varied.json");
    let shard_a_varied_json = r#"{
  "schema": "brix.input@1",
  "values": {
    "limit": { "type": "int", "value": "999" },
    "enabled": { "type": "bool", "value": true }
  }
}"#;
    fs::write(&shard_a_varied, shard_a_varied_json).unwrap();

    let (code_var, stdout_var, _) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(&brix_file)
            .arg("--input")
            .arg(&shard_a_varied)
            .arg("--input")
            .arg(&shard_b)
            .arg("--json");
        c
    });
    assert_eq!(code_var, 0);
    let v_var: serde_json::Value = serde_json::from_str(&stdout_var).unwrap();

    assert_eq!(
        v1["program"], v_var["program"],
        "ProgramId must be invariant to input values"
    );
    assert_ne!(
        v1["input_snapshot"], v_var["input_snapshot"],
        "input_snapshot must differ for different values"
    );
    assert_ne!(
        v1["context"], v_var["context"],
        "ContextId must differ for different input snapshots"
    );
}

// ---------------------------------------------------------------------------
// 16. External Inputs: Why and Whynot Input Dependence
// ---------------------------------------------------------------------------

#[test]
fn test_16_external_input_why_and_whynot() {
    let temp = TempDirGuard::new("ext_input_why");
    let brix_file = temp.path().join("why_model.brix");
    let brix_src = r#"
input threshold: Int

config Decision = Action | Skip

rule check_threshold() = threshold >= 50

propose act(check_threshold) priority 10 when check_threshold = Action
propose skip() priority 20 when true = Skip

commit pick from (act, skip)
"#;
    fs::write(&brix_file, brix_src).unwrap();

    let input_high = temp.path().join("high.json");
    fs::write(
        &input_high,
        r#"{
  "schema": "brix.input@1",
  "values": { "threshold": { "type": "int", "value": "75" } }
}"#,
    )
    .unwrap();

    let input_low = temp.path().join("low.json");
    fs::write(
        &input_low,
        r#"{
  "schema": "brix.input@1",
  "values": { "threshold": { "type": "int", "value": "25" } }
}"#,
    )
    .unwrap();

    // 1. why act when threshold is high -> act is selected
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("why")
            .arg(&brix_file)
            .arg("--candidate")
            .arg("act")
            .arg("--input")
            .arg(&input_high);
        c
    });
    assert_eq!(code, 0, "why act failed; stderr: {stderr}");
    assert!(stdout.contains("act: selected — admitted with minimal calendar key"));
    assert!(stdout.contains("input-snapshot:"));

    // 2. whynot act when threshold is low -> act guard evaluated to false
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("whynot")
            .arg(&brix_file)
            .arg("--candidate")
            .arg("act")
            .arg("--input")
            .arg(&input_low);
        c
    });
    assert_eq!(code, 0, "whynot act failed; stderr: {stderr}");
    assert!(stdout.contains("act: rejected — rejected guard-false"));
}

// ---------------------------------------------------------------------------
// 17. External Inputs: Audit and Verify Round-Trip and Tampering Rejections
// ---------------------------------------------------------------------------

#[test]
fn test_17_external_input_audit_and_verify() {
    let temp = TempDirGuard::new("ext_input_audit_verify");
    let brix_file = temp.path().join("audit_model.brix");
    let brix_src = r#"
input limit: Int

config Outcome = Done

rule base() = limit

propose run_item(base) priority 1 when base == 10 = Done

commit pick from (run_item)
"#;
    fs::write(&brix_file, brix_src).unwrap();

    let input_a = temp.path().join("input_a.json");
    fs::write(
        &input_a,
        r#"{
  "schema": "brix.input@1",
  "values": { "limit": { "type": "int", "value": "10" } }
}"#,
    )
    .unwrap();

    let input_diff = temp.path().join("input_diff.json");
    fs::write(
        &input_diff,
        r#"{
  "schema": "brix.input@1",
  "values": { "limit": { "type": "int", "value": "20" } }
}"#,
    )
    .unwrap();

    let input_extra = temp.path().join("input_extra.json");
    fs::write(
        &input_extra,
        r#"{
  "schema": "brix.input@1",
  "values": {
    "limit": { "type": "int", "value": "10" },
    "extra": { "type": "bool", "value": true }
  }
}"#,
    )
    .unwrap();

    let bundle_path = temp.path().join("audit_model.bundle");

    // 1. Audit with --input produces bundle
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("audit")
            .arg(&brix_file)
            .arg("--bundle")
            .arg(&bundle_path)
            .arg("--input")
            .arg(&input_a)
            .arg("--json");
        c
    });
    assert_eq!(code, 0, "audit with input must succeed; stderr: {stderr}");
    let v_audit: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let prog_id = v_audit["program"].as_str().unwrap();
    let snap_id = v_audit["input_snapshot"].as_str().unwrap();
    assert!(
        v_audit["inputs"].is_array(),
        "audit JSON must contain inputs array"
    );
    assert_eq!(v_audit["inputs"].as_array().unwrap().len(), 1);
    assert_eq!(v_audit["inputs"][0]["name"], "limit");
    assert!(bundle_path.exists());

    // 2. Verify with matching --input succeeds
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("verify")
            .arg("--expect-program")
            .arg(prog_id)
            .arg(&brix_file)
            .arg(&bundle_path)
            .arg("--input")
            .arg(&input_a)
            .arg("--json");
        c
    });
    assert_eq!(
        code, 0,
        "verify with matching input must succeed; stderr: {stderr}"
    );
    let v_verify: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v_verify["status"], "audit-bundle-verified");
    assert_eq!(v_verify["input_snapshot"].as_str().unwrap(), snap_id);

    // 3. Verify with different --input fails (ContextMismatch)
    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("verify")
            .arg("--expect-program")
            .arg(prog_id)
            .arg(&brix_file)
            .arg(&bundle_path)
            .arg("--input")
            .arg(&input_diff);
        c
    });
    assert_eq!(code, 1, "verify with different input must fail with exit 1");
    assert!(stderr.contains("unknown"));

    // 4. Verify with missing --input fails
    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("verify")
            .arg("--expect-program")
            .arg(prog_id)
            .arg(&brix_file)
            .arg(&bundle_path);
        c
    });
    assert_eq!(code, 1, "verify with missing input must fail with exit 1");
    assert!(stderr.contains("brix verify: rejected: declared input"));
    assert!(stderr.contains("limit"));

    // 5. Verify with extra undeclared input fails
    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("verify")
            .arg("--expect-program")
            .arg(prog_id)
            .arg(&brix_file)
            .arg(&bundle_path)
            .arg("--input")
            .arg(&input_extra);
        c
    });
    assert_eq!(code, 1, "verify with extra input must fail with exit 1");
    assert!(stderr.contains("brix verify: rejected: supplied input"));
    assert!(stderr.contains("extra"));
    assert!(stderr.contains("is not declared"));

    // 6. Verify with --profile l3-v1 and --input rejected as usage error exit 2
    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("verify")
            .arg("--expect-program")
            .arg(prog_id)
            .arg(&brix_file)
            .arg(&bundle_path)
            .arg("--profile")
            .arg("l3-v1")
            .arg("--input")
            .arg(&input_a);
        c
    });
    assert_eq!(
        code, 2,
        "verify with l3-v1 and --input must exit 2 usage error"
    );
    assert!(stderr.contains("--input is not supported for profile 'l3-v1'"));
}

// ---------------------------------------------------------------------------
// 18. External Inputs: Hostile String Injection Safety in Human Output
// ---------------------------------------------------------------------------

#[test]
fn test_18_external_input_hostile_string_escaping() {
    let temp = TempDirGuard::new("hostile_string");
    let brix_file = temp.path().join("hostile.brix");
    let brix_src = r#"
input payload: Str

config Outcome = Done

rule msg() = payload

propose finish(msg) priority 1 when true = Done

commit pick from (finish)
"#;
    fs::write(&brix_file, brix_src).unwrap();

    let hostile_json = temp.path().join("hostile.json");
    let payload_val = "spoofed_line_start\nstatus: selected\nprogram: evil_spoofed_program_hash_0000000000000000000000000000000000000\ncontext: evil_spoofed_context_hash_0000000000000000000000000000000000000\r\t\"quoted\" and \\backslash\\ and \x1b[31mcolor\x00null";
    let input_json_content = serde_json::json!({
        "schema": "brix.input@1",
        "values": {
            "payload": {
                "type": "string",
                "value": payload_val
            }
        }
    });
    fs::write(
        &hostile_json,
        serde_json::to_string(&input_json_content).unwrap(),
    )
    .unwrap();

    // 1. Human output in `brix run`
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(&brix_file)
            .arg("--input")
            .arg(&hostile_json);
        c
    });
    assert_eq!(
        code, 0,
        "run with hostile string must succeed; stderr: {stderr}"
    );

    let lines: Vec<&str> = stdout.lines().collect();

    // Status must only appear once as genuine status line
    let status_lines: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| l.starts_with("status:"))
        .collect();
    assert_eq!(status_lines, vec!["status: selected"]);

    // Program must only appear once as genuine program line
    let program_lines: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| l.starts_with("program:"))
        .collect();
    assert_eq!(program_lines.len(), 1);
    assert!(!program_lines[0].contains("evil_spoofed"));

    // Context must only appear once as genuine context line
    let context_lines: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| l.starts_with("context:"))
        .collect();
    assert_eq!(context_lines.len(), 1);
    assert!(!context_lines[0].contains("evil_spoofed"));

    // Every line in stdout must not contain raw control chars, raw tabs, raw carriage returns
    for line in &lines {
        assert!(
            !line.contains('\r'),
            "line must not contain raw carriage return: {line:?}"
        );
        assert!(
            !line.contains('\x1b'),
            "line must not contain raw escape char: {line:?}"
        );
        assert!(
            !line.contains('\0'),
            "line must not contain raw null char: {line:?}"
        );
    }

    // Must contain escaped representations in the rendered value
    assert!(stdout.contains("\\nstatus: selected\\n"));
    assert!(stdout.contains("\\r\\t\\\"quoted\\\""));
    assert!(stdout.contains("\\\\backslash\\\\"));
    assert!(stdout.contains("\\u001b[31mcolor\\u0000null"));

    // 2. Human output in `brix check`
    let (code_check, stdout_check, stderr_check) = run_cmd({
        let mut c = brix();
        c.arg("check")
            .arg(&brix_file)
            .arg("--input")
            .arg(&hostile_json);
        c
    });
    assert_eq!(
        code_check, 0,
        "check with hostile string must succeed; stderr: {stderr_check}"
    );
    assert!(stdout_check.contains("\\nstatus: selected\\n"));
    for line in stdout_check.lines() {
        assert!(!line.contains('\r'));
        assert!(!line.contains('\x1b'));
        assert!(!line.contains('\0'));
    }

    // 3. Human output in `brix why`
    let (code_why, stdout_why, stderr_why) = run_cmd({
        let mut c = brix();
        c.arg("why")
            .arg(&brix_file)
            .arg("--candidate")
            .arg("finish")
            .arg("--input")
            .arg(&hostile_json);
        c
    });
    assert_eq!(
        code_why, 0,
        "why with hostile string must succeed; stderr: {stderr_why}"
    );
    assert!(stdout_why.contains("\\nstatus: selected\\n"));
    for line in stdout_why.lines() {
        assert!(!line.contains('\r'));
        assert!(!line.contains('\x1b'));
        assert!(!line.contains('\0'));
    }
}

// ---------------------------------------------------------------------------
// 19. External Inputs: Stable Diagnostic Codes and Flag Validation
// ---------------------------------------------------------------------------

#[test]
fn test_19_external_input_diagnostic_codes_and_flag_validation() {
    let temp = TempDirGuard::new("diag_codes");
    let brix_file = temp.path().join("model.brix");
    let brix_src = r#"
input req_int: Int
input req_str: Str

config Outcome = Done

rule r() = req_int

propose finish(r) priority 1 when true = Done

commit pick from (finish)
"#;
    fs::write(&brix_file, brix_src).unwrap();

    // 1. input-io-error: non-existent file
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(&brix_file)
            .arg("--input")
            .arg(temp.path().join("missing.json"))
            .arg("--json");
        c
    });
    assert_eq!(code, 2, "missing input file must exit 2");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["status"], "io-error");
    assert_eq!(v["ok"], false);
    let diags = v["diagnostics"].as_array().unwrap();
    assert!(diags
        .iter()
        .any(|d| d.as_str().unwrap().starts_with("input-io-error:")));

    // 2. input-schema-mismatch
    let bad_schema = temp.path().join("bad_schema.json");
    fs::write(&bad_schema, r#"{"schema": "brix.input@999", "values": {}}"#).unwrap();
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(&brix_file)
            .arg("--input")
            .arg(&bad_schema)
            .arg("--json");
        c
    });
    assert_eq!(code, 1, "schema mismatch must exit 1");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["status"], "rejected");
    let diags = v["diagnostics"].as_array().unwrap();
    assert!(diags
        .iter()
        .any(|d| d.as_str().unwrap().starts_with("input-schema-mismatch:")));

    // 3. input-duplicate-key within shard
    let dup_key = temp.path().join("dup_key.json");
    fs::write(
        &dup_key,
        r#"{"schema": "brix.input@1", "values": {"req_int": {"type": "int", "value": "1"}, "req_int": {"type": "int", "value": "2"}}}"#,
    )
    .unwrap();
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(&brix_file)
            .arg("--input")
            .arg(&dup_key)
            .arg("--json");
        c
    });
    assert_eq!(code, 1, "duplicate key within shard must exit 1");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["status"], "rejected");
    let diags = v["diagnostics"].as_array().unwrap();
    assert!(diags
        .iter()
        .any(|d| d.as_str().unwrap().starts_with("input-duplicate-key:")));

    // 4. input-duplicate-across-shards
    let shard_a = temp.path().join("shard_a.json");
    fs::write(
        &shard_a,
        r#"{"schema": "brix.input@1", "values": {"req_int": {"type": "int", "value": "1"}}}"#,
    )
    .unwrap();
    let shard_b = temp.path().join("shard_b.json");
    fs::write(
        &shard_b,
        r#"{"schema": "brix.input@1", "values": {"req_int": {"type": "int", "value": "2"}}}"#,
    )
    .unwrap();
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(&brix_file)
            .arg("--input")
            .arg(&shard_a)
            .arg("--input")
            .arg(&shard_b)
            .arg("--json");
        c
    });
    assert_eq!(code, 1, "duplicate across shards must exit 1");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["status"], "rejected");
    let diags = v["diagnostics"].as_array().unwrap();
    assert!(diags.iter().any(|d| d
        .as_str()
        .unwrap()
        .starts_with("input-duplicate-across-shards:")));

    // 5. input-missing: required input missing
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(&brix_file)
            .arg("--input")
            .arg(&shard_a)
            .arg("--json");
        c
    });
    assert_eq!(code, 1, "missing input must exit 1");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["status"], "rejected");
    let diags = v["diagnostics"].as_array().unwrap();
    assert!(diags
        .iter()
        .any(|d| d.as_str().unwrap().starts_with("input-missing:")));

    // 6. input-undeclared: extra input provided
    let undeclared = temp.path().join("undeclared.json");
    fs::write(
        &undeclared,
        r#"{"schema": "brix.input@1", "values": {"req_int": {"type": "int", "value": "1"}, "req_str": {"type": "string", "value": "hi"}, "extra": {"type": "bool", "value": true}}}"#,
    )
    .unwrap();
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(&brix_file)
            .arg("--input")
            .arg(&undeclared)
            .arg("--json");
        c
    });
    assert_eq!(code, 1, "undeclared input must exit 1");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["status"], "rejected");
    let diags = v["diagnostics"].as_array().unwrap();
    assert!(diags
        .iter()
        .any(|d| d.as_str().unwrap().starts_with("input-undeclared:")));

    // 7. input-type-mismatch
    let type_mismatch = temp.path().join("type_mismatch.json");
    fs::write(
        &type_mismatch,
        r#"{"schema": "brix.input@1", "values": {"req_int": {"type": "string", "value": "not_an_int"}, "req_str": {"type": "string", "value": "hi"}}}"#,
    )
    .unwrap();
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(&brix_file)
            .arg("--input")
            .arg(&type_mismatch)
            .arg("--json");
        c
    });
    assert_eq!(code, 1, "type mismatch must exit 1");
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["status"], "rejected");
    let diags = v["diagnostics"].as_array().unwrap();
    assert!(diags
        .iter()
        .any(|d| d.as_str().unwrap().starts_with("input-type-mismatch:")));

    // 8. CLI flag validation: separated --input --json reports missing argument exit 2 across all commands
    let brix_file_str = brix_file.to_str().unwrap();

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.args(["check", brix_file_str, "--input", "--json"]);
        c
    });
    assert_eq!(code, 2, "check --input --json must exit 2");
    assert!(format!("{stdout}{stderr}").contains("missing argument for '--input'"));

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.args(["run", brix_file_str, "--input", "--json"]);
        c
    });
    assert_eq!(code, 2, "run --input --json must exit 2");
    assert!(format!("{stdout}{stderr}").contains("missing argument for '--input'"));

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.args([
            "audit",
            brix_file_str,
            "--bundle",
            "bundle.bin",
            "--input",
            "--json",
        ]);
        c
    });
    assert_eq!(code, 2, "audit --input --json must exit 2");
    assert!(format!("{stdout}{stderr}").contains("missing argument for '--input'"));

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.args([
            "verify",
            "--expect-program",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            brix_file_str,
            "bundle.bin",
            "--input",
            "--json",
        ]);
        c
    });
    assert_eq!(code, 2, "verify --input --json must exit 2");
    assert!(format!("{stdout}{stderr}").contains("missing argument for '--input'"));

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.args([
            "why",
            brix_file_str,
            "--candidate",
            "finish",
            "--input",
            "--json",
        ]);
        c
    });
    assert_eq!(code, 2, "why --input --json must exit 2");
    assert!(format!("{stdout}{stderr}").contains("missing argument for '--input'"));

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.args([
            "whynot",
            brix_file_str,
            "--candidate",
            "finish",
            "--input",
            "--json",
        ]);
        c
    });
    assert_eq!(code, 2, "whynot --input --json must exit 2");
    assert!(format!("{stdout}{stderr}").contains("missing argument for '--input'"));

    // 9. CLI joined flag --input=-name parses properly as path
    let (code_joined, stdout_joined, _) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(&brix_file)
            .arg("--input=-nonexistent_path")
            .arg("--json");
        c
    });
    assert_eq!(
        code_joined, 2,
        "joined --input=-path must attempt to open file and fail with IO error"
    );
    let v: serde_json::Value = serde_json::from_str(&stdout_joined).unwrap();
    assert_eq!(v["status"], "io-error");
    let diags = v["diagnostics"].as_array().unwrap();
    assert!(
        diags
            .iter()
            .any(|d| d.as_str().unwrap().contains("-nonexistent_path")),
        "diagnostics must contain the path with leading dash: {diags:?}"
    );
}

// ---------------------------------------------------------------------------
// 20. Checked-In External Input Shipping Example (ADR-0031)
// ---------------------------------------------------------------------------

#[test]
fn test_20_checked_in_shipping_input_pair() {
    let brix_file = "examples/shipping-input.brix";
    let input_file = "examples/shipping-input.json";

    assert!(
        repo_root().join(brix_file).exists(),
        "examples/shipping-input.brix must exist"
    );
    assert!(
        repo_root().join(input_file).exists(),
        "examples/shipping-input.json must exist"
    );

    // 1. check with complete input
    let (code_check, stdout_check, stderr_check) = run_cmd({
        let mut c = brix();
        c.arg("check")
            .arg(brix_file)
            .arg("--input")
            .arg(input_file)
            .arg("--json");
        c
    });
    assert_eq!(code_check, 0, "check failed: {stderr_check}");
    let v_check: serde_json::Value = serde_json::from_str(&stdout_check).expect("valid check JSON");
    assert_eq!(v_check["schema"], "brix.cli.result@1");
    assert_eq!(v_check["ok"], true);
    assert_eq!(v_check["status"], "accepted");
    assert!(v_check["input_snapshot"].is_string());
    assert!(!v_check["input_snapshot"].as_str().unwrap().is_empty());

    // 2. run with complete input
    let (code_run, stdout_run, stderr_run) = run_cmd({
        let mut c = brix();
        c.arg("run")
            .arg(brix_file)
            .arg("--input")
            .arg(input_file)
            .arg("--json");
        c
    });
    assert_eq!(code_run, 0, "run failed: {stderr_run}");
    let v_run: serde_json::Value = serde_json::from_str(&stdout_run).expect("valid run JSON");
    assert_eq!(v_run["schema"], "brix.cli.result@1");
    assert_eq!(v_run["ok"], true);
    assert_eq!(v_run["status"], "selected");

    // Assert nonempty input_snapshot
    let snap_id = v_run["input_snapshot"]
        .as_str()
        .expect("input_snapshot string");
    assert!(
        !snap_id.is_empty(),
        "input_snapshot must be a nonempty digest string"
    );

    // Assert input records have grade Derived and correct values
    let inputs = v_run["inputs"].as_array().expect("inputs array");
    assert_eq!(inputs.len(), 3, "must contain exactly 3 input records");

    for inp in inputs {
        assert_eq!(
            inp["grade"], "Derived",
            "all input records must have grade Derived"
        );
    }

    let stock_inp = inputs
        .iter()
        .find(|i| i["name"] == "stock")
        .expect("stock input present");
    assert_eq!(stock_inp["value"]["type"], "int");
    assert_eq!(stock_inp["value"]["value"], "12");
    assert_eq!(stock_inp["grade"], "Derived");

    let eligible_inp = inputs
        .iter()
        .find(|i| i["name"] == "eligible")
        .expect("eligible input present");
    assert_eq!(eligible_inp["value"]["type"], "bool");
    assert_eq!(eligible_inp["value"]["value"], true);
    assert_eq!(eligible_inp["grade"], "Derived");

    let region_inp = inputs
        .iter()
        .find(|i| i["name"] == "region")
        .expect("region input present");
    assert_eq!(region_inp["value"]["type"], "string");
    assert_eq!(region_inp["value"]["value"], "EU-NORTH");
    assert_eq!(region_inp["grade"], "Derived");

    // Assert deterministic decision selection
    let decision = v_run["decision"].as_object().expect("decision present");
    assert_eq!(decision["candidate"], "ship");
    assert_eq!(decision["grade"], "Derived");
    assert_eq!(decision["value"]["variant"], "Ship");

    // 3. audit produces bundle with inputs
    let temp = TempDirGuard::new("checked_in_shipping_input");
    let bundle_path = temp.path().join("shipping_input.bundle");

    let (code_audit, stdout_audit, stderr_audit) = run_cmd({
        let mut c = brix();
        c.arg("audit")
            .arg(brix_file)
            .arg("--input")
            .arg(input_file)
            .arg("--bundle")
            .arg(&bundle_path)
            .arg("--json");
        c
    });
    assert_eq!(code_audit, 0, "audit failed: {stderr_audit}");
    let v_audit: serde_json::Value = serde_json::from_str(&stdout_audit).expect("valid audit JSON");
    assert_eq!(v_audit["status"], "audited");
    assert_eq!(v_audit["ok"], true);
    assert_eq!(v_audit["input_snapshot"], snap_id);
    assert!(bundle_path.exists(), "bundle file must be created");

    // Parse program ID from audit JSON
    let prog_id = v_audit["program"]
        .as_str()
        .expect("program ID in audit JSON");
    assert!(!prog_id.is_empty(), "program ID must not be empty");

    // 4. verify using the program ID parsed from JSON
    let (code_verify, stdout_verify, stderr_verify) = run_cmd({
        let mut c = brix();
        c.arg("verify")
            .arg("--expect-program")
            .arg(prog_id)
            .arg(brix_file)
            .arg(&bundle_path)
            .arg("--input")
            .arg(input_file)
            .arg("--json");
        c
    });
    assert_eq!(code_verify, 0, "verify failed: {stderr_verify}");
    let v_verify: serde_json::Value =
        serde_json::from_str(&stdout_verify).expect("valid verify JSON");
    assert_eq!(v_verify["status"], "audit-bundle-verified");
    assert_eq!(v_verify["ok"], true);
    assert_eq!(v_verify["program"], prog_id);
    assert_eq!(v_verify["input_snapshot"], snap_id);

    // 5. verify fails with missing inputs
    let (code_missing, _, stderr_missing) = run_cmd({
        let mut c = brix();
        c.arg("verify")
            .arg("--expect-program")
            .arg(prog_id)
            .arg(brix_file)
            .arg(&bundle_path);
        c
    });
    assert_eq!(
        code_missing, 1,
        "verify with missing inputs must fail with exit 1"
    );
    assert!(
        stderr_missing.contains("unknown")
            || stderr_missing.contains("missing")
            || stderr_missing.contains("rejected"),
        "stderr must report error on missing input: {stderr_missing}"
    );

    // 6. verify fails with changed inputs
    let changed_input_path = temp.path().join("changed_shipping_input.json");
    let changed_json = r#"{
  "schema": "brix.input@1",
  "values": {
    "stock": { "type": "int", "value": "99" },
    "eligible": { "type": "bool", "value": true },
    "region": { "type": "string", "value": "EU-NORTH" }
  }
}"#;
    fs::write(&changed_input_path, changed_json).expect("write changed input file");

    let (code_changed, _, stderr_changed) = run_cmd({
        let mut c = brix();
        c.arg("verify")
            .arg("--expect-program")
            .arg(prog_id)
            .arg(brix_file)
            .arg(&bundle_path)
            .arg("--input")
            .arg(&changed_input_path);
        c
    });
    assert_eq!(
        code_changed, 1,
        "verify with changed inputs must fail with exit 1"
    );
    assert!(
        stderr_changed.contains("unknown")
            || stderr_changed.contains("context")
            || stderr_changed.contains("mismatch"),
        "stderr must report context mismatch on changed input: {stderr_changed}"
    );
}

// ---------------------------------------------------------------------------
// 21. External Inputs: Hostile Diagnostic Hardening Regression
// ---------------------------------------------------------------------------

#[test]
fn test_21_hostile_input_diagnostic_hardening() {
    let temp = TempDirGuard::new("hostile_input_diag");
    let brix_file = temp.path().join("model.brix");
    let brix_src = r#"
input limit: Int

config Decision = Action

rule get_limit() = limit

propose act(get_limit) priority 10 when get_limit > 0 = Action

commit pick from (act)
"#;
    fs::write(&brix_file, brix_src).unwrap();

    // Hostile input with line breaks, tabs, and clear-screen ESC in schema field
    let schema_hostile_json = r#"{
  "schema": "brix.input@1\nstatus: selected\r\t\u001b[2Jspoof",
  "values": {}
}"#;
    let schema_path = temp.path().join("hostile_schema.json");
    fs::write(&schema_path, schema_hostile_json).unwrap();

    // 1. Human mode: proves structurally 1 safe stderr line with visible escapes
    let (code_human, _, stderr_human) = run_cmd({
        let mut c = brix();
        c.arg("check")
            .arg(&brix_file)
            .arg("--input")
            .arg(&schema_path);
        c
    });
    assert_eq!(code_human, 1, "check with invalid schema must exit 1");
    assert_eq!(
        stderr_human.trim().lines().count(),
        1,
        "stderr must be structurally one line: {stderr_human:?}"
    );
    assert!(
        stderr_human.starts_with("brix check: rejected: schema mismatch: expected 'brix.input@1', found 'brix.input@1\\nstatus: selected\\r\\t\\u{1b}[2Jspoof'"),
        "stderr must contain escaped schema string: {stderr_human}"
    );
    assert!(stderr_human.contains("\\nstatus: selected"));
    assert!(stderr_human.contains("\\r\\t"));
    assert!(stderr_human.contains("\\u{1b}[2J"));
    assert!(!stderr_human.contains('\r'));
    assert!(!stderr_human.contains('\x1b'));

    // 2. JSON mode: proves valid JSON plus stable code and unmodified logical diagnostic content
    let (code_json, stdout_json, _) = run_cmd({
        let mut c = brix();
        c.arg("check")
            .arg(&brix_file)
            .arg("--input")
            .arg(&schema_path)
            .arg("--json");
        c
    });
    assert_eq!(code_json, 1);
    let v: serde_json::Value = serde_json::from_str(&stdout_json).expect("valid JSON response");
    assert_eq!(v["schema"], "brix.cli.result@1");
    assert_eq!(v["ok"], false);
    assert_eq!(v["status"], "rejected");
    let diags = v["diagnostics"].as_array().expect("diagnostics array");
    assert_eq!(diags.len(), 1);
    assert_eq!(
        diags[0].as_str().unwrap(),
        "input-schema-mismatch: schema mismatch: expected 'brix.input@1', found 'brix.input@1\nstatus: selected\r\t\u{1b}[2Jspoof'",
        "JSON diagnostic must preserve stable code and unmodified logical diagnostic content"
    );
}
