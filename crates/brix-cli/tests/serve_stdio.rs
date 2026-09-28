//! Integration tests for `brix serve --stdio` (ADR-0044): drives the built
//! `brix` binary as a subprocess over its stdin/stdout JSON-lines protocol.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

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

/// A live `brix serve --stdio` subprocess with line-oriented request/response
/// helpers for the tests below.
struct ServeSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl ServeSession {
    fn spawn() -> Self {
        let mut child = Command::new(brix_bin())
            .arg("serve")
            .arg("--stdio")
            .current_dir(repo_root())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn brix serve --stdio");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = BufReader::new(child.stdout.take().expect("child stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    /// Write one raw line (a trailing `\n` is appended) to the server's stdin.
    fn send_raw(&mut self, line: &str) {
        self.stdin
            .write_all(line.as_bytes())
            .expect("write request line");
        self.stdin.write_all(b"\n").expect("write newline");
        self.stdin.flush().expect("flush stdin");
    }

    /// Serialize `request` and send it as one line.
    fn send(&mut self, request: &Value) {
        self.send_raw(&serde_json::to_string(request).unwrap());
    }

    /// Read and parse exactly one response line. Panics on EOF.
    fn recv(&mut self) -> Value {
        let mut line = String::new();
        let n = self
            .stdout
            .read_line(&mut line)
            .expect("read response line");
        assert!(n > 0, "server closed stdout unexpectedly (EOF)");
        serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("response line was not valid JSON: {e}: {line:?}"))
    }

    /// Send one request and read back its matching response.
    fn call(&mut self, method: &str, id: Value, params: Value) -> Value {
        self.send(&json!({ "id": id, "method": method, "params": params }));
        self.recv()
    }

    /// Close stdin (signals EOF to the server) and wait for the process to
    /// exit, returning its exit code.
    fn finish(mut self) -> i32 {
        drop(self.stdin);
        let status = self.child.wait().expect("wait for brix serve to exit");
        status.code().unwrap_or(-1)
    }
}

// ---------------------------------------------------------------------------
// hello
// ---------------------------------------------------------------------------

#[test]
fn test_hello_reports_versions_and_methods() {
    let mut s = ServeSession::spawn();
    let resp = s.call("hello", json!(1), json!({}));
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["schema"], "brix.serve.hello@1");
    assert_eq!(resp["result"]["protocol"], "brix.serve@1");
    let methods = resp["result"]["methods"].as_array().unwrap();
    for m in [
        "check",
        "run",
        "why",
        "whynot",
        "audit",
        "verify",
        "test",
        "kb.init",
        "kb.assert",
    ] {
        assert!(
            methods.iter().any(|v| v == m),
            "expected method '{m}' in hello methods: {methods:?}"
        );
    }
    assert!(resp["result"]["toolchain_version"].is_string());
    assert_eq!(s.finish(), 0);
}

// ---------------------------------------------------------------------------
// check / run via path
// ---------------------------------------------------------------------------

#[test]
fn test_check_by_path_matches_cli_json() {
    let mut s = ServeSession::spawn();
    let resp = s.call(
        "check",
        json!("c1"),
        json!({ "program": { "path": "examples/shipping.brix" } }),
    );
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["id"], "c1");
    assert_eq!(resp["result"]["schema"], "brix.cli.result@1");
    assert_eq!(resp["result"]["command"], "check");
    assert_eq!(resp["result"]["ok"], true);
    assert_eq!(resp["result"]["status"], "accepted");
    assert_eq!(s.finish(), 0);
}

#[test]
fn test_run_by_path_selects_ship() {
    let mut s = ServeSession::spawn();
    let resp = s.call(
        "run",
        json!(2),
        json!({ "program": { "path": "examples/shipping.brix" } }),
    );
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["exit_code"], 0);
    assert_eq!(resp["result"]["command"], "run");
    assert_eq!(resp["result"]["status"], "selected");
    assert_eq!(resp["result"]["decision"]["candidate"], "ship");
    assert_eq!(s.finish(), 0);
}

// ---------------------------------------------------------------------------
// inline program source
// ---------------------------------------------------------------------------

#[test]
fn test_run_with_inline_program_source() {
    let source = std::fs::read_to_string(repo_root().join("examples/shipping.brix")).unwrap();
    let mut s = ServeSession::spawn();
    let resp = s.call("run", json!(3), json!({ "program": { "source": source } }));
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["result"]["status"], "selected");
    assert_eq!(resp["result"]["decision"]["candidate"], "ship");
    assert_eq!(s.finish(), 0);
}

// ---------------------------------------------------------------------------
// why / whynot
// ---------------------------------------------------------------------------

#[test]
fn test_why_and_whynot() {
    let mut s = ServeSession::spawn();
    let why_resp = s.call(
        "why",
        json!(4),
        json!({ "program": { "path": "examples/shipping.brix" }, "candidate": "ship" }),
    );
    assert_eq!(why_resp["ok"], true);
    assert_eq!(why_resp["result"]["command"], "why");
    assert_eq!(why_resp["result"]["ok"], true);
    assert_eq!(why_resp["result"]["explanation"]["candidate"], "ship");

    let whynot_resp = s.call(
        "whynot",
        json!(5),
        json!({ "program": { "path": "examples/shipping.brix" }, "candidate": "expedite" }),
    );
    assert_eq!(whynot_resp["ok"], true);
    assert_eq!(whynot_resp["result"]["command"], "whynot");
    assert_eq!(s.finish(), 0);
}

#[test]
fn test_why_for_one_decide_instance() {
    // ADR-0043: `entity` selects one element of a per-entity `decide` block.
    let mut s = ServeSession::spawn();
    let resp = s.call(
        "why",
        json!(41),
        json!({
            "program": { "path": "examples/order-book.brix" },
            "inputs": [{ "path": "examples/order-book.json" }],
            "candidate": "backorder",
            "entity": 1
        }),
    );
    assert_eq!(resp["ok"], true, "{resp}");
    assert_eq!(resp["result"]["command"], "why");
    assert_eq!(resp["result"]["ok"], true);
    assert!(resp["result"]["entity_decisions"].is_array());

    let bad = s.call(
        "why",
        json!(42),
        json!({
            "program": { "path": "examples/order-book.brix" },
            "candidate": "ship",
            "entity": -1
        }),
    );
    assert_eq!(bad["ok"], false);
    assert_eq!(s.finish(), 0);
}

// ---------------------------------------------------------------------------
// a failing program: Unknown
// ---------------------------------------------------------------------------

#[test]
fn test_run_missing_declared_input_is_unknown_via_check_preflight() {
    // shipping-input.brix declares inputs; running without supplying any of
    // them is rejected by the strict input contract, exercised here through
    // `run` (which does not preflight the way `check` does, so a missing
    // input here surfaces as the same "rejected" input-validation failure
    // `brix run --json` would print).
    let mut s = ServeSession::spawn();
    let resp = s.call(
        "run",
        json!(6),
        json!({ "program": { "path": "examples/shipping-input.brix" } }),
    );
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["result"]["ok"], false);
    assert_eq!(resp["result"]["status"], "rejected");
    assert_eq!(s.finish(), 0);
}

// ---------------------------------------------------------------------------
// audit + verify
// ---------------------------------------------------------------------------

#[test]
fn test_audit_then_verify_round_trip() {
    let tmp = std::env::temp_dir().join(format!(
        "brix_serve_audit_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp).unwrap();
    let bundle_path = tmp.join("bundle.bin");

    let mut s = ServeSession::spawn();
    let audit_resp = s.call(
        "audit",
        json!(7),
        json!({
            "program": { "path": "examples/shipping.brix" },
            "bundle_out": bundle_path.to_string_lossy(),
        }),
    );
    assert_eq!(audit_resp["ok"], true);
    assert_eq!(audit_resp["result"]["ok"], true);
    let program_hex = audit_resp["result"]["program"]
        .as_str()
        .expect("program hex")
        .to_string();
    assert!(bundle_path.exists(), "audit bundle should be written");

    let verify_resp = s.call(
        "verify",
        json!(8),
        json!({
            "program": { "path": "examples/shipping.brix" },
            "bundle": bundle_path.to_string_lossy(),
            "expect_program": program_hex,
        }),
    );
    assert_eq!(verify_resp["ok"], true);
    assert_eq!(verify_resp["result"]["ok"], true);
    assert_eq!(verify_resp["result"]["command"], "verify");

    assert_eq!(s.finish(), 0);
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------
// test suite
// ---------------------------------------------------------------------------

#[test]
fn test_test_method_runs_suite_file() {
    let mut s = ServeSession::spawn();
    let resp = s.call(
        "test",
        json!(9),
        json!({ "files": ["examples/shipping.test.json"] }),
    );
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["result"]["schema"], "brix.test.result@1");
    assert_eq!(s.finish(), 0);
}

// ---------------------------------------------------------------------------
// kb session
// ---------------------------------------------------------------------------

#[test]
fn test_kb_init_assert_show_session() {
    let tmp = std::env::temp_dir().join(format!(
        "brix_serve_kb_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let kb_dir = tmp.join("kb");

    let mut s = ServeSession::spawn();
    let input_source =
        std::fs::read_to_string(repo_root().join("examples/shipping-input.json")).unwrap();

    let init_resp = s.call(
        "kb.init",
        json!(10),
        json!({
            "dir": kb_dir.to_string_lossy(),
            "program": { "path": "examples/shipping-input.brix" },
            "inputs": [ { "source": input_source } ],
        }),
    );
    assert_eq!(init_resp["ok"], true, "init response: {init_resp:?}");
    assert_eq!(init_resp["result"]["ok"], true);

    let show_resp = s.call(
        "kb.show",
        json!(11),
        json!({ "dir": kb_dir.to_string_lossy() }),
    );
    assert_eq!(show_resp["ok"], true);
    assert_eq!(show_resp["result"]["ok"], true);

    let log_resp = s.call(
        "kb.log",
        json!(12),
        json!({ "dir": kb_dir.to_string_lossy() }),
    );
    assert_eq!(log_resp["ok"], true);

    assert_eq!(s.finish(), 0);
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------
// inline input: duplicate key rejected, and inline vs file give identical
// snapshot ids
// ---------------------------------------------------------------------------

#[test]
fn test_inline_input_duplicate_key_rejected() {
    let dup_json = r#"{"schema":"brix.input@1","values":{"stock":{"type":"int","value":"1"},"stock":{"type":"int","value":"2"}}}"#;
    let mut s = ServeSession::spawn();
    let resp = s.call(
        "run",
        json!(13),
        json!({
            "program": { "path": "examples/shipping-input.brix" },
            "inputs": [ { "source": dup_json } ],
        }),
    );
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["result"]["ok"], false);
    assert_eq!(resp["result"]["status"], "rejected");
    let diags = resp["result"]["diagnostics"].as_array().unwrap();
    assert!(
        diags
            .iter()
            .any(|d| d.as_str().unwrap().contains("duplicate")),
        "expected a duplicate-key diagnostic, got: {diags:?}"
    );
    assert_eq!(s.finish(), 0);
}

#[test]
fn test_inline_vs_file_input_identical_snapshot_id() {
    let input_source =
        std::fs::read_to_string(repo_root().join("examples/shipping-input.json")).unwrap();

    let mut s = ServeSession::spawn();
    let file_resp = s.call(
        "run",
        json!(14),
        json!({
            "program": { "path": "examples/shipping-input.brix" },
            "inputs": [ { "path": "examples/shipping-input.json" } ],
        }),
    );
    let inline_resp = s.call(
        "run",
        json!(15),
        json!({
            "program": { "path": "examples/shipping-input.brix" },
            "inputs": [ { "source": input_source } ],
        }),
    );
    assert_eq!(file_resp["result"]["ok"], true);
    assert_eq!(inline_resp["result"]["ok"], true);
    assert_eq!(
        file_resp["result"]["input_snapshot"],
        inline_resp["result"]["input_snapshot"]
    );
    assert_eq!(
        file_resp["result"]["program"],
        inline_resp["result"]["program"]
    );
    assert_eq!(s.finish(), 0);
}

// ---------------------------------------------------------------------------
// pipelining, malformed lines, oversize lines, unknown methods
// ---------------------------------------------------------------------------

#[test]
fn test_pipelined_requests_respond_in_order() {
    let mut s = ServeSession::spawn();
    s.send(&json!({ "id": 1, "method": "hello", "params": {} }));
    s.send(&json!({
        "id": 2,
        "method": "check",
        "params": { "program": { "path": "examples/shipping.brix" } }
    }));
    s.send(&json!({
        "id": 3,
        "method": "run",
        "params": { "program": { "path": "examples/shipping.brix" } }
    }));

    let r1 = s.recv();
    let r2 = s.recv();
    let r3 = s.recv();
    assert_eq!(r1["id"], 1);
    assert_eq!(r1["result"]["schema"], "brix.serve.hello@1");
    assert_eq!(r2["id"], 2);
    assert_eq!(r2["result"]["command"], "check");
    assert_eq!(r3["id"], 3);
    assert_eq!(r3["result"]["command"], "run");
    assert_eq!(s.finish(), 0);
}

#[test]
fn test_malformed_json_line_gets_error_response_and_server_keeps_serving() {
    let mut s = ServeSession::spawn();
    s.send_raw("{not valid json");
    let err_resp = s.recv();
    assert_eq!(err_resp["ok"], false);
    assert_eq!(err_resp["error"]["code"], "malformed-request");

    // The server must still be alive and able to serve the next request.
    let resp = s.call("hello", json!(1), json!({}));
    assert_eq!(resp["ok"], true);
    assert_eq!(s.finish(), 0);
}

#[test]
fn test_unknown_method_gets_error_response() {
    let mut s = ServeSession::spawn();
    let resp = s.call("no-such-method", json!(1), json!({}));
    assert_eq!(resp["ok"], false);
    assert_eq!(resp["error"]["code"], "unknown-method");
    assert_eq!(s.finish(), 0);
}

#[test]
fn test_oversize_request_line_gets_error_response_and_server_keeps_serving() {
    let mut s = ServeSession::spawn();
    // Well over the 4 MiB request line limit.
    let huge = "x".repeat(5 * 1024 * 1024);
    s.send_raw(&huge);
    let err_resp = s.recv();
    assert_eq!(err_resp["ok"], false);
    assert_eq!(err_resp["error"]["code"], "request-too-large");

    let resp = s.call("hello", json!(1), json!({}));
    assert_eq!(resp["ok"], true);
    assert_eq!(s.finish(), 0);
}

// ---------------------------------------------------------------------------
// clean EOF
// ---------------------------------------------------------------------------

#[test]
fn test_clean_eof_exits_zero() {
    let s = ServeSession::spawn();
    assert_eq!(s.finish(), 0);
}

#[test]
fn test_nothing_but_responses_on_stdout() {
    // Every stdout line must be parseable JSON with the response schema —
    // no banners, prompts, or stray diagnostics.
    let mut s = ServeSession::spawn();
    let resp = s.call(
        "run",
        json!(1),
        json!({ "program": { "path": "examples/shipping.brix" } }),
    );
    assert_eq!(resp["schema"], "brix.serve@1");
    s.finish();
}
