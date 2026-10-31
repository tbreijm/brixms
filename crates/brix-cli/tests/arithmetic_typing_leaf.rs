//! `brix check` output for the Stage D arithmetic typing leaf (ADR-0015 §5
//! Stage D). `brix prove` is removed and `brix why` explains finite decisions;
//! the typing explanation lives in `brix check`'s binding output.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn brix_bin() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_brix")
        .or_else(|| std::env::var_os("NEXTEST_BIN_EXE_brix"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_brix")))
}

fn check(name: &str, source: &str, json: bool) -> (i32, String) {
    let dir = std::env::temp_dir().join(format!(
        "brix_arith_leaf_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("m.brix");
    fs::write(&file, source).unwrap();
    let mut cmd = Command::new(brix_bin());
    cmd.arg("check").arg(&file);
    if json {
        cmd.arg("--json");
    }
    let out = cmd.output().expect("run brix");
    let _ = fs::remove_dir_all(&dir);
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
    )
}

#[test]
fn human_output_separates_the_kernel_checked_leaf_from_the_remaining_obligations() {
    let (code, out) = check("human", "let x = 1 + 2\n", false);
    assert_eq!(code, 0, "{out}");
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "  x : Int @Proven = 3");
    assert_eq!(lines[1], "    arithmetic input leaf: kernel-checked");
    assert_eq!(lines[2], "    arithmetic typing leaf: kernel-checked");
    assert_eq!(lines.len(), 3);
}

#[test]
fn bindings_without_a_closed_leaf_print_exactly_as_before() {
    let (code, out) = check("plain", "let y = 7\n", false);
    assert_eq!(code, 0);
    assert_eq!(out.trim_end(), "  y : Int @Proven = 7");
}

#[test]
fn json_adds_typing_additively_and_keeps_value_distinct_from_the_proof_claim() {
    let (code, out) = check("json", "let x = 1 + 2\nlet y = 7\n", true);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).expect("json");
    assert_eq!(v["schema"], "brix.cli.result@1");
    let bindings = v["bindings"].as_array().unwrap();

    let x = &bindings[0];
    assert_eq!(x["name"], "x");
    assert_eq!(x["grade"], "Proven");
    assert!(
        x["value"].is_object(),
        "the evaluated value stays its own field"
    );
    let typing = &x["typing"];
    let closed: Vec<&str> = typing["kernel_checked_leaves"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["generator"].as_str().unwrap())
        .collect();
    assert_eq!(closed, vec!["g_arith_input", "g_arith"]);
    let remaining: Vec<&str> = typing["remaining_obligations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["generator"].as_str().unwrap())
        .collect();
    assert!(remaining.is_empty());

    // Pre-existing bindings carry no new key.
    let y = &bindings[1];
    assert!(y.get("typing").is_none());
    assert_eq!(y["grade"], "Proven");
}
