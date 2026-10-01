//! Numeric values survive process boundaries, audit replay, and KB persistence.
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "brix_numeric_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("numeric.brix"),
            r#"
input measurement: F64
input price: Decimal
config Result = Numeric(F64, Decimal)
rule measured() = measurement * f64("2")
rule total() = price + decimal("0.2")
propose numeric(measured, total) priority 10 when measured > f64("0") = Numeric(measured, total)
commit pick from (numeric)
"#,
        )
        .unwrap();
        fs::write(
            root.join("input.json"),
            json!({"schema":"brix.input@4", "values":{
                "measurement":{"type":"f64","value":"1.25"},
                "price":{"type":"decimal","value":"0.1"}
            }})
            .to_string(),
        )
        .unwrap();
        Self(root)
    }
    fn run(&self, args: &[&str]) -> Value {
        let binary = std::env::var_os("CARGO_BIN_EXE_brix")
            .or_else(|| std::env::var_os("NEXTEST_BIN_EXE_brix"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_brix")));
        let output = Command::new(binary)
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn run_output(&self, args: &[&str]) -> std::process::Output {
        let binary = std::env::var_os("CARGO_BIN_EXE_brix")
            .or_else(|| std::env::var_os("NEXTEST_BIN_EXE_brix"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_brix")));
        Command::new(binary)
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn numeric_run_and_audit_verify_use_exact_string_payloads() {
    let f = Fixture::new();
    let args = ["run", "--json", "numeric.brix", "--input", "input.json"];
    let run = f.run(&args);
    assert_eq!(run, f.run(&args), "numeric replay must be deterministic");
    let facts = run["facts"].as_array().unwrap();
    assert!(
        facts
            .iter()
            .any(|fact| fact["value"] == json!({"type":"f64","value":"2.5"})),
        "{run}"
    );
    assert!(
        facts
            .iter()
            .any(|fact| fact["value"] == json!({"type":"decimal","value":"0.3"})),
        "{run}"
    );
    let audit = f.run(&[
        "audit",
        "--json",
        "numeric.brix",
        "--input",
        "input.json",
        "--bundle",
        "numeric.bundle",
    ]);
    let pin = audit["program"].as_str().unwrap();
    let verify = f.run(&[
        "verify",
        "--json",
        "--expect-program",
        pin,
        "numeric.brix",
        "numeric.bundle",
        "--input",
        "input.json",
    ]);
    assert_eq!(verify["status"], "audit-bundle-verified");
}

#[test]
fn numeric_kb_snapshot_persistence_and_revision_verification() {
    let f = Fixture::new();
    f.run(&[
        "kb",
        "init",
        "kb",
        "numeric.brix",
        "--input",
        "input.json",
        "--json",
    ]);
    let snapshots: Vec<_> = fs::read_dir(f.0.join("kb/snapshots"))
        .unwrap()
        .map(|p| p.unwrap().path())
        .collect();
    assert_eq!(snapshots.len(), 1);
    let stored: Value = serde_json::from_slice(&fs::read(&snapshots[0]).unwrap()).unwrap();
    assert_eq!(stored["schema"], "brix.input@4");
    assert_eq!(
        stored["values"]["price"],
        json!({"type":"decimal","value":"0.1"})
    );
    fs::write(
        f.0.join("correction.json"),
        json!({"schema":"brix.input@4", "values":{
            "price":{"type":"decimal","value":"9007199254740993.1"}
        }})
        .to_string(),
    )
    .unwrap();
    f.run(&["kb", "assert", "kb", "--input", "correction.json", "--json"]);
    let shown = f.run(&["kb", "show", "kb", "--json"]);
    assert!(shown.to_string().contains("9007199254740993.3"), "{shown}");
    f.run(&["kb", "verify", "kb", "--json"]);
}

#[test]
fn numeric_fault_in_second_commit_pool_fails_closed_and_emits_no_audit_bundle() {
    let f = Fixture::new();
    for (name, faulting_guard) in [
        (
            "divide_by_zero",
            r#"decimal("1") / decimal("0") > decimal("0")"#,
        ),
        (
            "inexact_decimal_division",
            r#"decimal("1") / decimal("3") > decimal("0")"#,
        ),
    ] {
        let source = format!(
            r#"
config First = Accepted | Rejected
config Second = Broken | Fallback
propose first priority 1 when true = Accepted
propose broken priority 1 when {faulting_guard} = Broken
propose fallback otherwise = Fallback
commit first_commit from (first)
commit second_commit from (broken, fallback)
"#
        );
        let program = format!("{name}.brix");
        fs::write(f.0.join(&program), source).unwrap();

        for command in ["run", "check"] {
            let output = f.run_output(&[command, "--json", &program]);
            assert_eq!(
                output.status.code(),
                Some(1),
                "{command} should fail closed for {name}: stdout={}, stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            let result: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(result["ok"], false, "{command} {name}: {result}");
            assert_eq!(result["status"], "unknown", "{command} {name}: {result}");
        }

        let bundle = format!("{name}.bundle");
        let output = f.run_output(&["audit", "--json", &program, "--bundle", &bundle]);
        assert_eq!(
            output.status.code(),
            Some(1),
            "audit should fail closed for {name}: stdout={}, stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["ok"], false, "audit {name}: {result}");
        assert_eq!(result["status"], "unknown", "audit {name}: {result}");
        assert!(
            !f.0.join(bundle).exists(),
            "audit must not leave a bundle after {name}"
        );
    }
}

#[test]
fn serve_stdio_roundtrips_numeric_input_and_fact_values() {
    let binary = std::env::var_os("CARGO_BIN_EXE_brix")
        .or_else(|| std::env::var_os("NEXTEST_BIN_EXE_brix"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_brix")));
    let mut child = Command::new(binary)
        .arg("serve")
        .arg("--stdio")
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn brix serve");

    let request = json!({
        "id": 77,
        "method": "run",
        "params": {
            "program": { "source": r#"
input measurement: F64
input price: Decimal
config Result = Done
rule doubled() = measurement * f64("2")
rule total() = price + decimal("0.20")
propose ready priority 1 when true = Done
commit result from (ready)
"# },
            "inputs": [{ "source": r#"
{"schema":"brix.input@4","values":{"measurement":{"type":"f64","value":"1.25"},"price":{"type":"decimal","value":"0.10"}}}
"# }]
        }
    });
    let mut stdin = child.stdin.take().expect("serve stdin");
    writeln!(stdin, "{}", serde_json::to_string(&request).unwrap()).unwrap();
    drop(stdin);

    let output = child.wait_with_output().expect("wait for serve");
    assert!(
        output.status.success(),
        "serve failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut lines = BufReader::new(output.stdout.as_slice()).lines();
    let response: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    assert_eq!(response["id"], 77);
    assert_eq!(response["ok"], true, "{response}");
    let facts = response["result"]["facts"].as_array().unwrap();
    assert!(
        facts
            .iter()
            .any(|fact| { fact["value"] == json!({"type":"f64", "value":"2.5"}) }),
        "{facts:?}"
    );
    assert!(
        facts
            .iter()
            .any(|fact| { fact["value"] == json!({"type":"decimal", "value":"0.3"}) }),
        "{facts:?}"
    );
}
