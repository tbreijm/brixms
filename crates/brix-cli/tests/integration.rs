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
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates parent")
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn brix() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brix"));
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
