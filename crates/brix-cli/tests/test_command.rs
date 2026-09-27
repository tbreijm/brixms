//! Binary integration tests for `brix test` (the "these inputs -> this decision" regression
//! test runner). Exercises the built CLI binary via `env!("CARGO_BIN_EXE_brix")`, mirroring the
//! conventions of `tests/integration.rs`, across:
//!
//! 1. every checked-in `examples/*.test.json` suite passes
//! 2. a deliberately wrong expectation fails with exit 1 and prints expected/actual
//! 3. malformed suite files (unknown key, duplicate key, missing file, empty cases, oversize
//!    file) all exit 2 with a clear diagnostic
//! 4. `--json` output shape (`brix.test.result@1`)
//! 5. paths inside a suite file resolve relative to the suite file's own directory, not the
//!    process's current working directory

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

/// A `brix` invocation with cwd fixed at the repo root, matching how the checked-in examples
/// reference each other with repo-relative paths.
fn brix() -> Command {
    let mut cmd = Command::new(brix_bin());
    cmd.current_dir(repo_root());
    cmd
}

/// A `brix` invocation with an explicit, caller-chosen cwd, for exercising path resolution that
/// must NOT depend on the process's working directory.
fn brix_in(dir: &Path) -> Command {
    let mut cmd = Command::new(brix_bin());
    cmd.current_dir(dir);
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
            "brix_test_cmd_{}_{}_{}",
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

/// A minimal, self-contained, input-free program that always selects candidate `yes` with
/// value `Yes`, for tests that only need a deterministic decision, not a specific policy.
const TINY_PROGRAM: &str = "\
config Decision = Yes | No

propose yes() priority 1 when true = Yes
propose never() priority 2 when false = No

commit outcome from (yes, never)
";

// ---------------------------------------------------------------------------
// 1. Every checked-in example suite passes.
// ---------------------------------------------------------------------------

#[test]
fn test_all_example_suites_pass() {
    let suites = [
        "examples/shipping.test.json",
        "examples/shipping-input.test.json",
        "examples/shipping-functions.test.json",
        "examples/order-policy.test.json",
        "examples/allocation.test.json",
    ];
    for suite in &suites {
        assert!(
            repo_root().join(suite).exists(),
            "expected example suite to exist: {suite}"
        );
    }

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("test");
        for suite in &suites {
            c.arg(suite);
        }
        c
    });
    assert_eq!(code, 0, "expected all example suites to pass: {stderr}");
    assert!(stdout.contains("test result:"), "stdout: {stdout}");
    assert!(stdout.contains(" passed, 0 failed"), "stdout: {stdout}");
    // Every case name from every suite should be reported ok.
    assert!(!stdout.contains("FAIL"), "unexpected failure: {stdout}");
}

#[test]
fn test_each_example_suite_individually() {
    let suites = [
        "examples/shipping.test.json",
        "examples/shipping-input.test.json",
        "examples/shipping-functions.test.json",
        "examples/order-policy.test.json",
        "examples/allocation.test.json",
    ];
    for suite in &suites {
        let (code, stdout, stderr) = run_cmd({
            let mut c = brix();
            c.arg("test").arg(suite);
            c
        });
        assert_eq!(
            code, 0,
            "suite {suite} failed: stdout={stdout} stderr={stderr}"
        );
        assert!(
            stdout.contains(" passed, 0 failed"),
            "suite {suite}: {stdout}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. A deliberately wrong expectation fails with exit 1 and shows expected/actual.
// ---------------------------------------------------------------------------

#[test]
fn test_wrong_expectation_fails_with_expected_and_actual() {
    let dir = TempDirGuard::new("wrong_expect");
    fs::write(dir.path().join("program.brix"), TINY_PROGRAM).unwrap();
    fs::write(
        dir.path().join("suite.test.json"),
        r#"{
            "schema": "brix.test@1",
            "program": "program.brix",
            "cases": [
                {
                    "name": "deliberately wrong decision and value",
                    "inputs": [],
                    "expect": {
                        "status": "selected",
                        "decision": "never",
                        "value": "No"
                    }
                }
            ]
        }"#,
    )
    .unwrap();

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("test").arg(dir.path().join("suite.test.json"));
        c
    });
    assert_eq!(code, 1, "stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains("FAIL deliberately wrong decision and value"),
        "{stdout}"
    );
    assert!(
        stdout.contains("decision: expected: never, actual: yes"),
        "{stdout}"
    );
    assert!(
        stdout.contains("value: expected: No, actual: Yes"),
        "{stdout}"
    );
    assert!(
        stdout.contains("test result: 0 passed, 1 failed"),
        "{stdout}"
    );
}

#[test]
fn test_passing_and_failing_cases_in_same_suite_mix_correctly() {
    let dir = TempDirGuard::new("mixed");
    fs::write(dir.path().join("program.brix"), TINY_PROGRAM).unwrap();
    fs::write(
        dir.path().join("suite.test.json"),
        r#"{
            "schema": "brix.test@1",
            "program": "program.brix",
            "cases": [
                {"name": "correct", "expect": {"status": "selected", "decision": "yes", "value": "Yes"}},
                {"name": "incorrect", "expect": {"status": "selected", "decision": "never"}}
            ]
        }"#,
    )
    .unwrap();

    let (code, stdout, _stderr) = run_cmd({
        let mut c = brix();
        c.arg("test").arg(dir.path().join("suite.test.json"));
        c
    });
    assert_eq!(code, 1);
    assert!(stdout.contains("ok   correct"), "{stdout}");
    assert!(stdout.contains("FAIL incorrect"), "{stdout}");
    assert!(
        stdout.contains("test result: 1 passed, 1 failed"),
        "{stdout}"
    );
}

// ---------------------------------------------------------------------------
// 3. Malformed suite files exit 2.
// ---------------------------------------------------------------------------

#[test]
fn test_unknown_key_exits_2() {
    let dir = TempDirGuard::new("unknown_key");
    fs::write(dir.path().join("program.brix"), TINY_PROGRAM).unwrap();
    fs::write(
        dir.path().join("suite.test.json"),
        r#"{
            "schema": "brix.test@1",
            "program": "program.brix",
            "bogus_field": true,
            "cases": [{"name": "c", "expect": {"status": "selected"}}]
        }"#,
    )
    .unwrap();

    let (code, _stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("test").arg(dir.path().join("suite.test.json"));
        c
    });
    assert_eq!(code, 2, "stderr={stderr}");
    assert!(stderr.contains("unknown field 'bogus_field'"), "{stderr}");
}

#[test]
fn test_duplicate_key_exits_2() {
    let dir = TempDirGuard::new("dup_key");
    fs::write(dir.path().join("program.brix"), TINY_PROGRAM).unwrap();
    fs::write(
        dir.path().join("suite.test.json"),
        r#"{
            "schema": "brix.test@1",
            "program": "program.brix",
            "program": "program.brix",
            "cases": [{"name": "c", "expect": {"status": "selected"}}]
        }"#,
    )
    .unwrap();

    let (code, _stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("test").arg(dir.path().join("suite.test.json"));
        c
    });
    assert_eq!(code, 2, "stderr={stderr}");
    assert!(stderr.contains("duplicate JSON key 'program'"), "{stderr}");
}

#[test]
fn test_duplicate_key_in_nested_object_exits_2() {
    let dir = TempDirGuard::new("dup_key_nested");
    fs::write(dir.path().join("program.brix"), TINY_PROGRAM).unwrap();
    fs::write(
        dir.path().join("suite.test.json"),
        r#"{
            "schema": "brix.test@1",
            "program": "program.brix",
            "cases": [{
                "name": "c",
                "expect": {
                    "status": "selected",
                    "candidates": {"yes": "selected", "yes": "rejected"}
                }
            }]
        }"#,
    )
    .unwrap();

    let (code, _stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("test").arg(dir.path().join("suite.test.json"));
        c
    });
    assert_eq!(code, 2, "stderr={stderr}");
    assert!(stderr.contains("duplicate JSON key 'yes'"), "{stderr}");
}

#[test]
fn test_missing_suite_file_exits_2() {
    let dir = TempDirGuard::new("missing_file");
    let missing = dir.path().join("does-not-exist.test.json");

    let (code, _stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("test").arg(&missing);
        c
    });
    assert_eq!(code, 2, "stderr={stderr}");
    assert!(stderr.contains("cannot read"), "{stderr}");
}

#[test]
fn test_missing_program_file_exits_2() {
    let dir = TempDirGuard::new("missing_program");
    fs::write(
        dir.path().join("suite.test.json"),
        r#"{
            "schema": "brix.test@1",
            "program": "does-not-exist.brix",
            "cases": [{"name": "c", "expect": {"status": "selected"}}]
        }"#,
    )
    .unwrap();

    let (code, _stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("test").arg(dir.path().join("suite.test.json"));
        c
    });
    assert_eq!(code, 2, "stderr={stderr}");
    assert!(stderr.contains("cannot prepare program"), "{stderr}");
}

#[test]
fn test_empty_cases_exits_2() {
    let dir = TempDirGuard::new("empty_cases");
    fs::write(dir.path().join("program.brix"), TINY_PROGRAM).unwrap();
    fs::write(
        dir.path().join("suite.test.json"),
        r#"{"schema": "brix.test@1", "program": "program.brix", "cases": []}"#,
    )
    .unwrap();

    let (code, _stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("test").arg(dir.path().join("suite.test.json"));
        c
    });
    assert_eq!(code, 2, "stderr={stderr}");
    assert!(stderr.contains("must not be empty"), "{stderr}");
}

#[test]
fn test_oversize_suite_file_exits_2() {
    let dir = TempDirGuard::new("oversize");
    fs::write(dir.path().join("program.brix"), TINY_PROGRAM).unwrap();

    // Pad well past the 1 MiB source-file bound with a long JSON string value.
    let padding = "x".repeat(2 * 1024 * 1024);
    let oversized = format!(
        r#"{{"schema": "brix.test@1", "program": "program.brix", "cases": [{{"name": "{padding}", "expect": {{"status": "selected"}}}}]}}"#
    );
    fs::write(dir.path().join("suite.test.json"), oversized).unwrap();

    let (code, _stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("test").arg(dir.path().join("suite.test.json"));
        c
    });
    assert_eq!(code, 2, "stderr={stderr}");
    assert!(stderr.contains("exceeds maximum size limit"), "{stderr}");
}

#[test]
fn test_invalid_status_value_exits_2() {
    let dir = TempDirGuard::new("invalid_status");
    fs::write(dir.path().join("program.brix"), TINY_PROGRAM).unwrap();
    fs::write(
        dir.path().join("suite.test.json"),
        r#"{"schema": "brix.test@1", "program": "program.brix", "cases": [{"name": "c", "expect": {"status": "maybe"}}]}"#,
    )
    .unwrap();

    let (code, _stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("test").arg(dir.path().join("suite.test.json"));
        c
    });
    assert_eq!(code, 2, "stderr={stderr}");
    assert!(
        stderr.contains("invalid 'expect.status' value 'maybe'"),
        "{stderr}"
    );
}

// ---------------------------------------------------------------------------
// 4. `--json` output shape.
// ---------------------------------------------------------------------------

#[test]
fn test_json_output_shape_on_success() {
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("test")
            .arg("examples/allocation.test.json")
            .arg("--json");
        c
    });
    assert_eq!(code, 0, "stderr={stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(v["schema"], "brix.test.result@1");
    assert_eq!(v["ok"], true);
    assert_eq!(v["failed"], 0);
    assert!(v["passed"].as_u64().unwrap() > 0);
    let files = v["files"].as_array().expect("files array");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["file"], "examples/allocation.test.json");
    assert_eq!(files[0]["program"], "allocation.brix");
    let cases = files[0]["cases"].as_array().expect("cases array");
    assert!(!cases.is_empty());
    for case in cases {
        assert!(case["name"].is_string());
        assert!(case["ok"].is_boolean());
        assert!(case["program"].is_string(), "case JSON: {case}");
        assert!(case["mismatches"].as_array().unwrap().is_empty());
    }
}

#[test]
fn test_json_output_shape_on_failure_includes_mismatches() {
    let dir = TempDirGuard::new("json_fail");
    fs::write(dir.path().join("program.brix"), TINY_PROGRAM).unwrap();
    fs::write(
        dir.path().join("suite.test.json"),
        r#"{
            "schema": "brix.test@1",
            "program": "program.brix",
            "cases": [
                {"name": "wrong", "expect": {"status": "quiescent"}}
            ]
        }"#,
    )
    .unwrap();

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("test")
            .arg(dir.path().join("suite.test.json"))
            .arg("--json");
        c
    });
    assert_eq!(code, 1, "stderr={stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(v["schema"], "brix.test.result@1");
    assert_eq!(v["ok"], false);
    assert_eq!(v["passed"], 0);
    assert_eq!(v["failed"], 1);
    let case = &v["files"][0]["cases"][0];
    assert_eq!(case["ok"], false);
    let mismatches = case["mismatches"].as_array().expect("mismatches array");
    assert_eq!(mismatches.len(), 1);
    assert_eq!(mismatches[0]["field"], "status");
    assert_eq!(mismatches[0]["expected"], "quiescent");
    assert_eq!(mismatches[0]["actual"], "selected");
}

#[test]
fn test_json_fatal_error_shape_on_malformed_file() {
    let dir = TempDirGuard::new("json_fatal");
    fs::write(
        dir.path().join("suite.test.json"),
        r#"{"schema": "brix.test@1", "program": "p.brix", "cases": []}"#,
    )
    .unwrap();

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("test")
            .arg(dir.path().join("suite.test.json"))
            .arg("--json");
        c
    });
    assert_eq!(code, 2, "stderr={stderr}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(v["schema"], "brix.test.result@1");
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap().contains("must not be empty"));
}

// ---------------------------------------------------------------------------
// 5. Paths inside a suite resolve relative to the suite file's own directory.
// ---------------------------------------------------------------------------

#[test]
fn test_paths_resolve_relative_to_suite_file_not_cwd() {
    let dir = TempDirGuard::new("relative_resolution");
    let sub = dir.path().join("suite_dir");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("program.brix"), TINY_PROGRAM).unwrap();
    fs::write(
        sub.join("suite.test.json"),
        r#"{
            "schema": "brix.test@1",
            "program": "program.brix",
            "cases": [
                {"name": "resolves relative to suite dir", "expect": {"status": "selected", "decision": "yes", "value": "Yes"}}
            ]
        }"#,
    )
    .unwrap();

    // Invoke with an unrelated cwd (the repo root) and an absolute path to the suite file: the
    // suite's own "program.brix" must resolve against `sub`, not against the repo root cwd.
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix(); // cwd = repo_root()
        c.arg("test").arg(sub.join("suite.test.json"));
        c
    });
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains("ok   resolves relative to suite dir"),
        "{stdout}"
    );

    // Invoke again with yet another, entirely different cwd, using a *relative* argument (from
    // that cwd) to reach the suite file: still must resolve program.brix against the suite
    // file's own directory, not this third cwd.
    let another_cwd = TempDirGuard::new("relative_resolution_cwd");
    let relative_from_another_cwd = pathdiff(&sub.join("suite.test.json"), another_cwd.path());
    let (code2, stdout2, stderr2) = run_cmd({
        let mut c = brix_in(another_cwd.path());
        c.arg("test").arg(&relative_from_another_cwd);
        c
    });
    assert_eq!(code2, 0, "stdout={stdout2} stderr={stderr2}");
    assert!(
        stdout2.contains("ok   resolves relative to suite dir"),
        "{stdout2}"
    );
}

/// Compute a relative path from `from` to `target`, for two absolute paths that share a common
/// ancestor. Used only to build a realistic "relative argument from an unrelated cwd" for
/// [`test_paths_resolve_relative_to_suite_file_not_cwd`]; not a general-purpose path utility.
fn pathdiff(target: &Path, from: &Path) -> PathBuf {
    let target_components: Vec<_> = target.components().collect();
    let from_components: Vec<_> = from.components().collect();
    let common = target_components
        .iter()
        .zip(from_components.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut result = PathBuf::new();
    for _ in common..from_components.len() {
        result.push("..");
    }
    for component in &target_components[common..] {
        result.push(component.as_os_str());
    }
    result
}

#[test]
fn test_package_paths_resolve_relative_to_suite_file() {
    let dir = TempDirGuard::new("package_paths");
    // A program that imports a package declared under a package root next to the suite file.
    let pkg_root = dir.path().join("pkgs");
    let pkg_src_dir = pkg_root.join("acme.util").join("src");
    fs::create_dir_all(&pkg_src_dir).unwrap();
    fs::write(pkg_src_dir.join("util.brix"), "config Marker = Present\n").unwrap();

    fs::write(
        dir.path().join("program.brix"),
        "use acme.util\n\nconfig Decision = Yes | No\n\npropose yes() priority 1 when true = Yes\n\ncommit outcome from (yes)\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("suite.test.json"),
        r#"{
            "schema": "brix.test@1",
            "program": "program.brix",
            "package_paths": ["pkgs"],
            "cases": [
                {"name": "imports resolve via suite-relative package_paths", "expect": {"status": "selected", "decision": "yes"}}
            ]
        }"#,
    )
    .unwrap();

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("test").arg(dir.path().join("suite.test.json"));
        c
    });
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(
        stdout.contains("ok   imports resolve via suite-relative package_paths"),
        "{stdout}"
    );
}

#[test]
fn test_multiple_suite_files_in_one_invocation() {
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("test")
            .arg("examples/shipping.test.json")
            .arg("examples/order-policy.test.json");
        c
    });
    assert_eq!(code, 0, "stdout={stdout} stderr={stderr}");
    assert!(stdout.contains("examples/shipping.test.json:"), "{stdout}");
    assert!(
        stdout.contains("examples/order-policy.test.json:"),
        "{stdout}"
    );
    assert!(
        stdout.contains("test result: 4 passed, 0 failed"),
        "{stdout}"
    );
}
