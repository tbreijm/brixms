//! Public CLI contract for the compiler-grounded, fail-closed quality gate.

use std::process::Command;

use camino::Utf8PathBuf;

const VALID: &str = "package smoke.quality @ 0.1.0\n\
rel Input { value: I64 } key(value)\n";

fn tmp_dir(tag: &str) -> Utf8PathBuf {
    let mut path =
        Utf8PathBuf::from_path_buf(std::env::temp_dir()).expect("temp path must be UTF-8");
    path.push(format!(
        "brix-cli-quality-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    path
}

fn brix(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_brix"))
        .args(args)
        .output()
        .expect("brix binary must be spawnable")
}

#[test]
fn quality_is_public_and_fails_closed_with_structured_evidence() {
    let help = brix(&["--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("brix quality <path>"));

    let root = tmp_dir("unavailable");
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("world.brix");
    std::fs::write(&source, VALID).unwrap();

    let output = brix(&[
        "quality",
        source.as_str(),
        "--profile",
        "production",
        "--diagnostic-format",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    let json = String::from_utf8_lossy(&output.stdout);
    assert!(json.starts_with("{\"diagnostics\":"), "{json}");
    assert!(json.contains("BRX-QUALITY-0001"), "{json}");
    assert!(json.contains("production"), "{json}");
    assert!(json.contains("\"static_checks\":\"passed\""), "{json}");
    assert!(!root.join(".brix-cache").exists());

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn quality_preserves_compiler_diagnostics_before_its_gate() {
    let root = tmp_dir("compiler-error");
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("broken.brix");
    std::fs::write(&source, "package broken @ 0.1.0\nrel Input {").unwrap();

    let output = brix(&["quality", source.as_str(), "--diagnostic-format=json"]);
    assert_eq!(output.status.code(), Some(1));
    let json = String::from_utf8_lossy(&output.stdout);
    assert!(json.contains("BRX-AST-"), "{json}");
    assert!(!json.contains("BRX-QUALITY-0001"), "{json}");

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn quality_rejects_invalid_invocations_as_usage_errors() {
    let missing = brix(&["quality"]);
    assert_eq!(missing.status.code(), Some(2));

    let bad_profile = brix(&["quality", "world.brix", "--profile", "strict"]);
    assert_eq!(bad_profile.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad_profile.stderr).contains("unsupported profile"));

    let bad_format = brix(&["quality", "world.brix", "--diagnostic-format", "xml"]);
    assert_eq!(bad_format.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad_format.stderr).contains("unsupported diagnostic format"));

    let extra = brix(&["quality", "one.brix", "two.brix"]);
    assert_eq!(extra.status.code(), Some(2));

    let unknown = brix(&["quality", "world.brix", "--approve"]);
    assert_eq!(unknown.status.code(), Some(2));
}
