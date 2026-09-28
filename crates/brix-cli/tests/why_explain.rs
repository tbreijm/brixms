//! Integration tests for `brix why`/`whynot`'s structured derivation
//! explanation (ADR-0030): the `because:` tree in human output, and the
//! additive `explanation` field in `--json` output.

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
            "brix_why_explain_{}_{}_{}",
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
// shipping.brix — plain guard trace, no inputs, selection line
// ---------------------------------------------------------------------------

#[test]
fn test_why_shipping_human_because_tree() {
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("whynot")
            .arg("examples/shipping.brix")
            .arg("--candidate")
            .arg("expedite");
        c
    });
    assert_eq!(code, 0, "whynot expedite failed: {stderr}");
    // Every existing line stays present and unchanged.
    assert!(stdout.contains("expedite: rejected — rejected guard-false"));
    assert!(stdout.contains("facts:\n  stock: 12 @Derived\n  threshold: 10 @Derived\n"));
    assert!(stdout.contains("decision: ship = Ship @Derived\nstatus: selected\n"));
    // The appended, indented `because:` tree.
    assert!(stdout.contains("because:\n"));
    assert!(stdout.contains("  guard: stock >= 50 => false\n"));
    assert!(stdout.contains("    stock => 12 @Derived\n"));
    assert!(stdout.contains("    50 => 50\n"));
    assert!(stdout.contains("  facts:\n    stock => 12 @Derived\n"));
    assert!(stdout.contains("  value: Expedite => Expedite\n"));
    // Rejected (never admitted) candidates carry no selection comparison.
    assert!(!stdout.contains("  selection:"));

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("why")
            .arg("examples/shipping.brix")
            .arg("--candidate")
            .arg("ship");
        c
    });
    assert_eq!(code, 0, "why ship failed: {stderr}");
    assert!(stdout.contains("ship: selected — admitted with minimal calendar key"));
    assert!(stdout.contains("  guard: stock >= threshold => true\n"));
    assert!(stdout.contains("    threshold => 10 @Derived\n"));
    assert!(stdout.contains("  value: Ship => Ship\n"));
    assert!(stdout
        .contains("  selection: priority 10 — least calendar key among admitted candidates\n"));

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("whynot")
            .arg("examples/shipping.brix")
            .arg("--candidate")
            .arg("hold");
        c
    });
    assert_eq!(code, 0, "whynot hold failed: {stderr}");
    assert!(stdout.contains(
        "selection: priority 100 vs winner 'ship' priority 10 — winner leads on priority\n"
    ));
}

#[test]
fn test_why_shipping_json_explanation_field() {
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("why")
            .arg("--json")
            .arg("examples/shipping.brix")
            .arg("--candidate")
            .arg("ship");
        c
    });
    assert_eq!(code, 0, "why ship --json failed: {stderr}");
    let val: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(val["schema"], "brix.cli.result@1");
    assert_eq!(val["ok"], true);
    let expl = &val["explanation"];
    assert_eq!(expl["candidate"], "ship");
    assert_eq!(expl["guard"]["source"], "stock >= threshold");
    assert_eq!(expl["guard"]["outcome"]["kind"], "value");
    assert_eq!(expl["guard"]["outcome"]["value"]["type"], "bool");
    assert_eq!(expl["guard"]["outcome"]["value"]["value"], true);
    assert_eq!(expl["guard"]["children"].as_array().unwrap().len(), 2);
    assert_eq!(expl["value"]["source"], "Ship");
    assert_eq!(expl["truncated"], false);
    let facts = expl["facts"].as_array().expect("facts array");
    assert!(facts.iter().any(|f| f["name"] == "stock"));
    assert!(facts.iter().any(|f| f["name"] == "threshold"));
    let selection = &expl["selection"];
    assert_eq!(selection["is_winner"], true);
    assert_eq!(selection["candidate"], "ship");

    // A candidate never admitted carries no `selection` key at all.
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("whynot")
            .arg("--json")
            .arg("examples/shipping.brix")
            .arg("--candidate")
            .arg("expedite");
        c
    });
    assert_eq!(code, 0, "whynot expedite --json failed: {stderr}");
    let val: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert!(val["explanation"]["selection"].is_null());
}

// ---------------------------------------------------------------------------
// shipping-input.brix — short-circuit `match` case with external inputs
// ---------------------------------------------------------------------------

#[test]
fn test_whynot_shipping_input_match_not_evaluated() {
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("whynot")
            .arg("examples/shipping-input.brix")
            .arg("--input")
            .arg("examples/shipping-input.json")
            .arg("--candidate")
            .arg("expedite");
        c
    });
    assert_eq!(code, 0, "whynot expedite failed: {stderr}");
    assert!(stdout.contains("expedite: rejected — rejected guard-false"));
    assert!(stdout.contains("because:\n"));
    // stock=12 is well under 50, so expedite's guard is a plain comparison.
    assert!(stdout.contains("  guard: stock >= 50 => false\n"));

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("why")
            .arg("--json")
            .arg("examples/shipping-input.brix")
            .arg("--input")
            .arg("examples/shipping-input.json")
            .arg("--candidate")
            .arg("ship");
        c
    });
    assert_eq!(code, 0, "why ship failed: {stderr}");
    let val: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let facts = val["explanation"]["facts"].as_array().expect("facts array");
    let can_ship = facts
        .iter()
        .find(|f| f["name"] == "can_ship")
        .expect("can_ship fact present");
    assert_eq!(can_ship["value"]["type"], "bool");
    assert_eq!(can_ship["value"]["value"], true);
    // eligible=true and valid_destination=true in the fixture input, so the
    // outer match's `false` arm must be reported not evaluated.
    let trace = &can_ship["trace"];
    let children = trace["children"].as_array().expect("match children");
    let not_evaluated = children
        .iter()
        .find(|c| c["outcome"]["kind"] == "not_evaluated");
    assert!(
        not_evaluated.is_some(),
        "expected an untaken match arm reported not evaluated: {trace}"
    );
}

// ---------------------------------------------------------------------------
// shipping-functions.brix — helper call case
// ---------------------------------------------------------------------------

#[test]
fn test_why_shipping_functions_helper_call_trace() {
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("whynot")
            .arg("examples/shipping-functions.brix")
            .arg("--input")
            .arg("examples/shipping-functions.json")
            .arg("--candidate")
            .arg("expedite");
        c
    });
    assert_eq!(code, 0, "whynot expedite failed: {stderr}");
    assert!(stdout.contains("because:\n"));
    assert!(stdout.contains("  guard: both(expedited, valid_dest) => false\n"));
    // The helper body is expanded one level inline.
    assert!(stdout.contains("match a { true => b, false => false }"));

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("why")
            .arg("--json")
            .arg("examples/shipping-functions.brix")
            .arg("--input")
            .arg("examples/shipping-functions.json")
            .arg("--candidate")
            .arg("ship");
        c
    });
    assert_eq!(code, 0, "why ship failed: {stderr}");
    let val: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let guard = &val["explanation"]["guard"];
    assert_eq!(guard["source"], "both(has_stock, valid_dest)");
    let children = guard["children"].as_array().expect("call children");
    // Two arguments plus the expanded body of `both`.
    assert_eq!(children.len(), 3);
    let body = &children[2];
    assert_eq!(body["source"], "match a { true => b, false => false }");
}

// ---------------------------------------------------------------------------
// allocation.brix — `&&` / `!` case
// ---------------------------------------------------------------------------

#[test]
fn test_whynot_allocation_and_not_trace() {
    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("whynot")
            .arg("examples/allocation.brix")
            .arg("--input")
            .arg("examples/allocation.json")
            .arg("--candidate")
            .arg("insufficient");
        c
    });
    assert_eq!(code, 0, "whynot insufficient failed: {stderr}");
    assert!(stdout.contains("because:\n"));
    assert!(stdout.contains("  guard: evenly_split && (!meets_minimum) => false\n"));
    assert!(stdout.contains("    evenly_split => true @Derived\n"));
    assert!(stdout.contains("    !meets_minimum => false\n"));
    assert!(stdout.contains("      meets_minimum => true @Derived\n"));

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("why")
            .arg("--json")
            .arg("examples/allocation.brix")
            .arg("--input")
            .arg("examples/allocation.json")
            .arg("--candidate")
            .arg("balanced");
        c
    });
    assert_eq!(code, 0, "why balanced failed: {stderr}");
    let val: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let guard = &val["explanation"]["guard"];
    assert_eq!(guard["source"], "evenly_split && meets_minimum");
    assert_eq!(guard["outcome"]["value"]["value"], true);
    let facts = val["explanation"]["facts"].as_array().expect("facts array");
    // `evenly_split`/`meets_minimum` transitively read `leftover`/`share`,
    // which themselves read the `batch` input — every level is present.
    for name in [
        "evenly_split",
        "meets_minimum",
        "leftover",
        "share",
        "batch",
    ] {
        assert!(
            facts.iter().any(|f| f["name"] == name),
            "expected fact '{name}' in transitively-expanded facts list"
        );
    }
    let batch = facts.iter().find(|f| f["name"] == "batch").unwrap();
    assert_eq!(batch["origin"]["kind"], "input");
    assert!(batch["trace"].is_null(), "an input has no further trace");
}

// ---------------------------------------------------------------------------
// Truncation
// ---------------------------------------------------------------------------

fn balanced_sum(n: usize) -> String {
    if n <= 1 {
        "1".to_string()
    } else {
        let left = n / 2;
        let right = n - left;
        format!("({} + {})", balanced_sum(left), balanced_sum(right))
    }
}

#[test]
fn test_why_truncation_marker_on_oversized_rule_body() {
    let temp = TempDirGuard::new("truncation");
    let brix_file = temp.path().join("big.brix");
    // 300 leaves => 599 AST nodes in this one rule body, well over the
    // explanation's 512-node budget while staying inside the plan's own
    // expression node/depth limits.
    let expr = balanced_sum(300);
    let source = format!(
        r#"
config Decision = Yes | No

rule big() = {expr}

propose p(big) priority 10 when big >= 0 = Yes
propose q() priority 100 when true = No

commit c from (p, q)
"#
    );
    fs::write(&brix_file, source).expect("write fixture");

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("why").arg(&brix_file).arg("--candidate").arg("p");
        c
    });
    assert_eq!(code, 0, "why p failed: {stderr}");
    assert!(stdout.contains("because:\n"));
    assert!(stdout.contains("  (truncated: node budget exhausted)\n"));

    let (code, stdout, stderr) = run_cmd({
        let mut c = brix();
        c.arg("why")
            .arg("--json")
            .arg(&brix_file)
            .arg("--candidate")
            .arg("p");
        c
    });
    assert_eq!(code, 0, "why p --json failed: {stderr}");
    let val: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(val["explanation"]["truncated"], true);

    fn contains_truncated(node: &serde_json::Value) -> bool {
        node["outcome"]["kind"] == "truncated"
            || node["children"]
                .as_array()
                .is_some_and(|cs| cs.iter().any(contains_truncated))
    }
    let facts = val["explanation"]["facts"].as_array().expect("facts array");
    let big_fact = facts
        .iter()
        .find(|f| f["name"] == "big")
        .expect("big fact present");
    assert_eq!(big_fact["value"]["value"], "300");
    assert!(
        contains_truncated(&big_fact["trace"]),
        "expected an explicit truncation marker somewhere in the oversized trace"
    );
}
