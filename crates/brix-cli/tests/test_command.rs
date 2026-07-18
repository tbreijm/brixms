//! Public contract for the compiler-grounded, fail-closed `brix test` verb.

use std::process::{Command, Output};

use camino::Utf8PathBuf;

const SCENARIO_FIXTURE: &str = "package smoke.test @ 0.1.0\n\
rel Input { value: I64 } key(value)\n\
scenario Smoke {\n\
  seed 1\n\
  assert at end { true }\n\
}\n";

fn tmp_dir(tag: &str) -> Utf8PathBuf {
    let mut path =
        Utf8PathBuf::from_path_buf(std::env::temp_dir()).expect("temp dir must be UTF-8");
    path.push(format!(
        "brix-cli-test-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    path
}

fn brix(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_brix"))
        .args(args)
        .output()
        .expect("brix binary must be spawnable")
}

#[test]
fn test_runs_compiler_then_fails_closed_when_execution_is_unavailable() {
    let root = tmp_dir("unavailable");
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("world.brix");
    std::fs::write(&source, SCENARIO_FIXTURE).unwrap();

    let output = brix(&["test", source.as_str(), "Smoke"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("BRX-TEST-0001"), "{stderr}");
    assert!(stderr.contains("not yet implemented"), "{stderr}");
    assert!(!root.join(".brix-cache").exists());

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn test_preserves_compiler_diagnostics_in_json() {
    let root = tmp_dir("compiler-diagnostic");
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("broken.brix");
    std::fs::write(&source, "package broken.test @ 0.1.0\nrel Input {").unwrap();

    let output = brix(&["test", source.as_str(), "--diagnostic-format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("{\"diagnostics\":"), "{stdout}");
    assert!(stdout.contains("BRX-AST-"), "{stdout}");
    assert!(!stdout.contains("BRX-TEST-0001"), "{stdout}");

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn test_unavailable_diagnostic_is_machine_readable_and_records_evidence() {
    let root = tmp_dir("json-evidence");
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("world.brix");
    std::fs::write(&source, SCENARIO_FIXTURE).unwrap();

    let output = brix(&["test", source.as_str(), "Smoke", "--diagnostic-format=json"]);
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("BRX-TEST-0001"), "{stdout}");
    assert!(
        stdout.contains("\"compiler_check_passed\":true"),
        "{stdout}"
    );
    assert!(stdout.contains("\"execution_available\":false"), "{stdout}");
    assert!(
        stdout.contains("\"discovered_scenarios\":[\"Smoke\"]"),
        "{stdout}"
    );
    assert!(stdout.contains("\"selectors\":[\"Smoke\"]"), "{stdout}");

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn test_reports_usage_errors_and_is_listed_in_help() {
    let missing = brix(&["test"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing.stderr).contains("expected a source file"));

    let format = brix(&["test", "ignored.brix", "--diagnostic-format", "yaml"]);
    assert_eq!(format.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&format.stderr).contains("unsupported diagnostic format"));

    let option = brix(&["test", "ignored.brix", "--write"]);
    assert_eq!(option.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&option.stderr).contains("unsupported option"));

    let help = brix(&["--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("brix test <path> [selector ...]"));
}
