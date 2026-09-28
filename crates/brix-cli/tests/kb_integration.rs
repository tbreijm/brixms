//! Binary integration tests for `brix kb <op>` (ADR-0041).
//!
//! Spawns the real `brix` executable, exactly like `tests/integration.rs`,
//! covering the full lifecycle: init → assert → retract → program → log →
//! show → diff → audit → verify, plus tamper detection, lock contention, and
//! strict decoding — and confirms an audit bundle produced by `brix kb audit`
//! verifies with the ordinary `brix verify` against the knowledge base's own
//! stored program and snapshot files.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
            "brix_kb_it_{}_{}_{}",
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

fn write_shard(dir: &Path, name: &str, json: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, json).expect("write shard");
    path
}

fn program_file(kb_dir: &Path, program_id_hex: &str) -> PathBuf {
    kb_dir
        .join("programs")
        .join(format!("{program_id_hex}.brix"))
}

fn snapshot_file(kb_dir: &Path, snapshot_id_hex: &str) -> PathBuf {
    kb_dir
        .join("snapshots")
        .join(format!("{snapshot_id_hex}.json"))
}

// ---------------------------------------------------------------------------
// help mentions kb
// ---------------------------------------------------------------------------

#[test]
fn test_help_mentions_kb() {
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("--help");
        c
    });
    assert_eq!(code, 0);
    assert!(stdout.contains("kb init"));
    assert!(stdout.contains("kb assert"));
    assert!(stdout.contains("kb retract"));
    assert!(stdout.contains("kb program"));
    assert!(stdout.contains("kb log"));
    assert!(stdout.contains("kb show"));
    assert!(stdout.contains("kb diff"));
    assert!(stdout.contains("kb audit"));
    assert!(stdout.contains("kb verify"));
}

// ---------------------------------------------------------------------------
// Full lifecycle
// ---------------------------------------------------------------------------

#[test]
fn test_init_assert_retract_program_log_show_diff_lifecycle() {
    let tmp = TempDirGuard::new("lifecycle");
    let kb_dir = tmp.path().join("kb");

    // init
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("kb")
            .arg("init")
            .arg(&kb_dir)
            .arg("examples/shipping-input.brix")
            .arg("--input")
            .arg("examples/shipping-input.json");
        c
    });
    assert_eq!(code, 0, "init should succeed: {stdout}");
    assert!(stdout.contains("revision: 1"));
    assert!(stdout.contains("decision: ship = Ship"));

    // a second init is refused, exit 2 (usage/IO)
    let (code, _stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("kb")
            .arg("init")
            .arg(&kb_dir)
            .arg("examples/shipping-input.brix");
        c
    });
    assert_eq!(code, 2);
    assert!(stderr.contains("already"));

    // assert: correct `stock` downward
    let corrected = write_shard(
        tmp.path(),
        "correction.json",
        r#"{"schema":"brix.input@1","values":{"stock":{"type":"int","value":"3"}}}"#,
    );
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("kb")
            .arg("assert")
            .arg(&kb_dir)
            .arg("--input")
            .arg(&corrected);
        c
    });
    assert_eq!(code, 0, "assert should succeed: {stdout}");
    assert!(stdout.contains("revision: 2"));
    assert!(stdout.contains("decision: hold = Hold"));

    // retract: makes the input contract incomplete — exit 1, honest outcome
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("kb").arg("retract").arg(&kb_dir).arg("stock");
        c
    });
    assert_eq!(
        code, 1,
        "retract to an incomplete contract should exit 1: {stdout}"
    );
    assert!(stdout.contains("revision: 3"));
    assert!(stdout.contains("status: missing-inputs"));
    assert!(stdout.contains("missing: stock"));

    // retracting an already-unset name is a usage error, exit 2, no new revision
    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("kb").arg("retract").arg(&kb_dir).arg("stock");
        c
    });
    assert_eq!(code, 2);
    assert!(stderr.contains("not currently set") || stderr.contains("not set"));

    // program: switch to a program with an unrelated input contract
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("kb")
            .arg("program")
            .arg(&kb_dir)
            .arg("examples/order-policy.brix");
        c
    });
    assert_eq!(
        code, 1,
        "program change landing on missing-inputs should exit 1: {stdout}"
    );
    assert!(stdout.contains("revision: 4"));
    assert!(stdout.contains("dropped"));

    // log: 4 revisions, oldest first
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("kb").arg("log").arg(&kb_dir);
        c
    });
    assert_eq!(code, 0);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 4);
    assert!(lines[0].starts_with("1: init"));
    assert!(lines[1].starts_with("2: assert(stock)"));
    assert!(lines[2].starts_with("3: retract(stock)"));
    assert!(lines[3].starts_with("4: program"));

    // show defaults to HEAD (revision 4)
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("kb").arg("show").arg(&kb_dir);
        c
    });
    // `show` reports a revision faithfully regardless of its status — it is a
    // read, not a fresh claim — so it exits 0 even though HEAD is missing-inputs.
    assert_eq!(code, 0);
    assert!(stdout.contains("revision: 4"));
    assert!(stdout.contains("status: missing-inputs"));

    // show an explicit historical revision
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("kb").arg("show").arg(&kb_dir).arg("--rev").arg("1");
        c
    });
    assert_eq!(code, 0);
    assert!(stdout.contains("revision: 1"));
    assert!(stdout.contains("decision: ship = Ship"));

    // diff 1 -> 2, with a "why" chain: stock changes can_ship, threshold untouched
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("kb").arg("diff").arg(&kb_dir).arg("1").arg("2");
        c
    });
    assert_eq!(code, 0, "diff should succeed: {stdout}");
    assert!(stdout.contains("~ stock: 12 -> 3"));
    assert!(stdout.contains("~ can_ship: true -> false"));
    assert!(stdout.contains("why: input(s) stock"));
    assert!(
        !stdout.contains("threshold"),
        "unchanged fact must not appear:\n{stdout}"
    );
    assert!(stdout.contains("decision: ship=Ship (selected) -> hold=Hold (selected)"));
}

// ---------------------------------------------------------------------------
// audit bundle verifies with plain `brix verify`
// ---------------------------------------------------------------------------

#[test]
fn test_kb_audit_bundle_verifies_with_brix_verify() {
    let tmp = TempDirGuard::new("audit_verify");
    let kb_dir = tmp.path().join("kb");
    let bundle_path = tmp.path().join("bundle.bin");

    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("kb")
            .arg("init")
            .arg(&kb_dir)
            .arg("examples/shipping-input.brix")
            .arg("--input")
            .arg("examples/shipping-input.json");
        c
    });
    assert_eq!(code, 0, "{stdout}");
    let program_id = stdout
        .lines()
        .find_map(|l| l.strip_prefix("program: "))
        .expect("program id line")
        .to_string();
    let snapshot_id = stdout
        .lines()
        .find_map(|l| l.strip_prefix("snapshot: "))
        .expect("snapshot id line")
        .to_string();

    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("kb")
            .arg("audit")
            .arg(&kb_dir)
            .arg("--rev")
            .arg("1")
            .arg("--bundle")
            .arg(&bundle_path);
        c
    });
    assert_eq!(code, 0, "kb audit should succeed: {stdout}");
    assert!(bundle_path.exists());

    let program_path = program_file(&kb_dir, &program_id);
    let snapshot_path = snapshot_file(&kb_dir, &snapshot_id);
    assert!(program_path.exists());
    assert!(snapshot_path.exists());

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("verify")
            .arg("--expect-program")
            .arg(&program_id)
            .arg(&program_path)
            .arg(&bundle_path)
            .arg("--input")
            .arg(&snapshot_path);
        c
    });
    assert_eq!(
        code, 0,
        "brix verify should accept a kb-audited bundle: {stdout} {stderr}"
    );
    assert!(stdout.contains("status: audit-bundle-verified"));
}

// ---------------------------------------------------------------------------
// tamper detection
// ---------------------------------------------------------------------------

fn init_single_revision(kb_dir: &Path) {
    let (code, _, _) = run_cmd({
        let mut c = brix();
        c.arg("kb")
            .arg("init")
            .arg(kb_dir)
            .arg("examples/shipping-input.brix")
            .arg("--input")
            .arg("examples/shipping-input.json");
        c
    });
    assert_eq!(code, 0);
}

#[test]
fn test_verify_fails_after_editing_revision_file() {
    let tmp = TempDirGuard::new("tamper_rev");
    let kb_dir = tmp.path().join("kb");
    init_single_revision(&kb_dir);

    let rev_path = kb_dir.join("revisions").join("1.json");
    let text = fs::read_to_string(&rev_path).unwrap();
    let tampered = text.replace("\"selected\"", "\"quiescent\"");
    fs::write(&rev_path, tampered).unwrap();

    let (code, _stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("kb").arg("verify").arg(&kb_dir);
        c
    });
    assert_eq!(code, 1);
    assert!(stderr.contains("unknown"));
}

#[test]
fn test_verify_fails_after_editing_snapshot_or_program_file() {
    let tmp = TempDirGuard::new("tamper_files");
    let kb_dir = tmp.path().join("kb");
    init_single_revision(&kb_dir);

    // Find the stored snapshot and program files.
    let snapshots_dir = kb_dir.join("snapshots");
    let snapshot_entry = fs::read_dir(&snapshots_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let text = fs::read_to_string(&snapshot_entry).unwrap();
    fs::write(&snapshot_entry, text.replace("\"12\"", "\"999\"")).unwrap();

    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("kb").arg("verify").arg(&kb_dir);
        c
    });
    assert_eq!(code, 1);
    assert!(stderr.contains("snapshot"));
}

// ---------------------------------------------------------------------------
// lock contention
// ---------------------------------------------------------------------------

#[test]
fn test_lock_contention_exits_usage_error() {
    let tmp = TempDirGuard::new("lock");
    let kb_dir = tmp.path().join("kb");
    fs::create_dir_all(&kb_dir).unwrap();
    fs::write(kb_dir.join(".lock"), b"pid=999999").unwrap();

    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("kb")
            .arg("init")
            .arg(&kb_dir)
            .arg("examples/shipping-input.brix");
        c
    });
    assert_eq!(code, 2);
    assert!(stderr.contains("locked"));

    fs::remove_file(kb_dir.join(".lock")).unwrap();
    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("kb")
            .arg("init")
            .arg(&kb_dir)
            .arg("examples/shipping-input.brix")
            .arg("--input")
            .arg("examples/shipping-input.json");
        c
    });
    assert_eq!(
        code, 0,
        "init should succeed once the lock is released: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// strict decoding
// ---------------------------------------------------------------------------

#[test]
fn test_strict_decoding_rejects_duplicate_key_in_revision_file() {
    let tmp = TempDirGuard::new("strict_dup");
    let kb_dir = tmp.path().join("kb");
    init_single_revision(&kb_dir);

    let rev_path = kb_dir.join("revisions").join("1.json");
    let text = fs::read_to_string(&rev_path).unwrap();
    let tampered = text.replacen("\"seq\": 1,", "\"seq\": 1,\n  \"seq\": 1,", 1);
    fs::write(&rev_path, tampered).unwrap();

    let (code, _, stderr) = run_cmd({
        let mut c = brix();
        c.arg("kb").arg("show").arg(&kb_dir);
        c
    });
    assert_eq!(code, 1);
    assert!(stderr.contains("duplicate"));
}

// ---------------------------------------------------------------------------
// --json shape
// ---------------------------------------------------------------------------

#[test]
fn test_json_output_is_valid_json_for_init_and_log() {
    let tmp = TempDirGuard::new("json");
    let kb_dir = tmp.path().join("kb");

    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("kb")
            .arg("init")
            .arg(&kb_dir)
            .arg("examples/shipping-input.brix")
            .arg("--input")
            .arg("examples/shipping-input.json")
            .arg("--json");
        c
    });
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(v["command"], "kb init");
    assert_eq!(v["ok"], true);
    assert_eq!(v["revision"]["seq"], 1);

    let (code, stdout, _) = run_cmd({
        let mut c = brix();
        c.arg("kb").arg("log").arg(&kb_dir).arg("--json");
        c
    });
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(v["revisions"].as_array().unwrap().len(), 1);
}
