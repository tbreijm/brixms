//! `brix serve --stdio` — a long-running JSON-lines protocol over stdin/stdout
//! (ADR-0044), letting a caller drive the exact same command pipeline as the
//! CLI without parsing human-oriented text.
//!
//! # Framing
//! One JSON object per line on stdin, one JSON object per line on stdout.
//! Requests carry an `id` (any JSON value) that is echoed back verbatim on
//! the matching response, so a caller can pipeline several requests without
//! waiting for each response before sending the next. Processing itself is
//! strictly sequential — one request handled fully (including any file I/O)
//! before the next is read — so responses are always emitted in the same
//! order requests were read, which is the only ordering a caller needs.
//!
//! # Schema
//! Both the request and response envelopes are versioned `brix.serve@1`
//! (see [`SERVE_SCHEMA`]). A request is `{"id", "method", "params"}`; a
//! response is `{"schema", "id", "ok", "exit_code"?, "result"?, "error"?}`.
//! `ok: true` means the method was dispatched and produced a result —
//! *not* that the underlying command itself succeeded: a rejected/unknown
//! `check` or `run` is still `ok: true` at the protocol level, with its
//! failure recorded inside `result` (`result.ok: false`, `result.status`,
//! `exit_code`) exactly as `--json` would print it. `ok: false` means a
//! protocol-level problem — malformed JSON, an unknown method, or bad
//! params — prevented any command from running at all.
//!
//! Every method's `result` is exactly the JSON object the equivalent CLI
//! invocation would print with `--json`, obtained via
//! [`crate::json::with_captured_result`] rather than any re-implementation:
//! `check`/`run`/`why`/`whynot`/`audit`/`verify` share `brix.cli.result@1`;
//! `test` uses `brix.test.result@1`; `kb.*` uses `brix.cli.kb-result@1`. This
//! module never constructs those result objects itself.
//!
//! # Programs and inputs
//! A program or an input file can be given either by path (resolved exactly
//! like a CLI operand/`--input` argument, relative to the server process's
//! current working directory) or inline, as a JSON *string* holding the raw
//! source/envelope text (`{"source": "..."}` instead of `{"path": "..."}`).
//! Carrying inline input as a JSON string — not a nested JSON object — means
//! decoding it is a single, lossless JSON-string-literal decode of the outer
//! request line; the embedded `brix.input@N` text then reaches the existing
//! strict file decoder exactly as sent, byte for byte, with duplicate keys
//! still rejected. Inline text is materialized into a private per-request
//! temporary directory (removed once the request completes) and then read
//! through the same bounded file-reading code the CLI uses, so inline and
//! file-based sources that carry the same bytes produce identical results,
//! including identical snapshot/program identities.
//!
//! # Limits
//! A request line over [`MAX_REQUEST_LINE_BYTES`] is rejected without ever
//! being buffered in full (`error.code = "request-too-large"`). Program and
//! input resource limits are exactly the CLI's own (`ParseLimits::strict()`,
//! `InputLimits::default()`) — nothing here loosens or duplicates them.
//!
//! # Errors and shutdown
//! A malformed request line (invalid JSON, missing/wrong-typed fields, an
//! unknown method, bad params) yields one `ok: false` error response and the
//! server keeps serving subsequent lines. Clean EOF on stdin ends the loop
//! and the process exits 0. Nothing but protocol response lines is ever
//! written to stdout; anything else (this module never does so in normal
//! operation) would corrupt the framing for the caller.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli::{KbOp, VerifyProfile};
use crate::json::with_captured_result;

/// Schema identifier for both the request and response envelopes.
pub const SERVE_SCHEMA: &str = "brix.serve@1";

/// Maximum accepted length, in bytes, of one request line (4 MiB).
pub const MAX_REQUEST_LINE_BYTES: usize = 4 * 1024 * 1024;

/// Every method name `brix serve --stdio` accepts, in the order `hello`
/// reports them.
const SUPPORTED_METHODS: &[&str] = &[
    "hello",
    "check",
    "run",
    "why",
    "whynot",
    "audit",
    "verify",
    "test",
    "kb.init",
    "kb.assert",
    "kb.retract",
    "kb.program",
    "kb.log",
    "kb.show",
    "kb.diff",
    "kb.audit",
    "kb.verify",
];

/// Run the `brix serve --stdio` loop against `stdin`/`stdout`, returning the
/// process exit code (`0` on clean EOF; nonzero only for an unrecoverable
/// local I/O failure writing a response, which is otherwise never expected).
pub fn run_stdio() -> u8 {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    loop {
        match read_bounded_line(&mut input, MAX_REQUEST_LINE_BYTES) {
            Ok(ReadLineResult::Eof) => return crate::cli::EXIT_SUCCESS,
            Ok(ReadLineResult::Line(line)) => {
                if line.trim().is_empty() {
                    // Blank lines between requests are tolerated silently
                    // (no response is defined for "nothing"): common when a
                    // caller writes lines with a trailing newline convention.
                    continue;
                }
                let response = handle_line(&line);
                if write_response(&mut out, &response).is_err() {
                    return crate::cli::EXIT_USAGE_OR_IO;
                }
            }
            Ok(ReadLineResult::TooLarge) => {
                let response = error_response(
                    Value::Null,
                    "request-too-large",
                    format!("request line exceeds {MAX_REQUEST_LINE_BYTES} bytes"),
                );
                if write_response(&mut out, &response).is_err() {
                    return crate::cli::EXIT_USAGE_OR_IO;
                }
            }
            Ok(ReadLineResult::InvalidUtf8) => {
                let response = error_response(
                    Value::Null,
                    "invalid-utf8",
                    "request line is not valid UTF-8 text".to_string(),
                );
                if write_response(&mut out, &response).is_err() {
                    return crate::cli::EXIT_USAGE_OR_IO;
                }
            }
            Err(_) => return crate::cli::EXIT_USAGE_OR_IO,
        }
    }
}

fn write_response(out: &mut impl Write, response: &Value) -> std::io::Result<()> {
    let text = serde_json::to_string(response).expect("response value always serializes");
    out.write_all(text.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()
}

// ---------------------------------------------------------------------------
// Bounded line reading
// ---------------------------------------------------------------------------

enum ReadLineResult {
    Eof,
    Line(String),
    TooLarge,
    InvalidUtf8,
}

/// Read one `\n`-terminated line (tolerating a preceding `\r`) from `reader`,
/// never buffering more than `max_len` bytes even for a hostile, arbitrarily
/// long line: once the running total exceeds `max_len` the partial buffer is
/// dropped and the rest of the line is still drained (bounded chunk by
/// chunk via the reader's own internal buffer) so framing stays intact for
/// the next line.
fn read_bounded_line(reader: &mut impl BufRead, max_len: usize) -> std::io::Result<ReadLineResult> {
    let mut buf: Vec<u8> = Vec::new();
    let mut too_large = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if buf.is_empty() && !too_large {
                return Ok(ReadLineResult::Eof);
            }
            break;
        }
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            if !too_large {
                if buf.len().saturating_add(pos) > max_len {
                    too_large = true;
                    buf.clear();
                } else {
                    buf.extend_from_slice(&available[..pos]);
                }
            }
            reader.consume(pos + 1);
            break;
        }
        let n = available.len();
        if !too_large {
            if buf.len().saturating_add(n) > max_len {
                too_large = true;
                buf.clear();
            } else {
                buf.extend_from_slice(available);
            }
        }
        reader.consume(n);
    }
    if too_large {
        return Ok(ReadLineResult::TooLarge);
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    match String::from_utf8(buf) {
        Ok(s) => Ok(ReadLineResult::Line(s)),
        Err(_) => Ok(ReadLineResult::InvalidUtf8),
    }
}

// ---------------------------------------------------------------------------
// Request / response envelopes
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Request {
    #[serde(default)]
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}

fn ok_response(id: Value, exit_code: u8, result: Value) -> Value {
    json!({
        "schema": SERVE_SCHEMA,
        "id": id,
        "ok": true,
        "exit_code": exit_code,
        "result": result,
    })
}

fn error_response(id: Value, code: &str, message: String) -> Value {
    json!({
        "schema": SERVE_SCHEMA,
        "id": id,
        "ok": false,
        "error": { "code": code, "message": message },
    })
}

fn handle_line(line: &str) -> Value {
    let request: Request = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(err) => {
            return error_response(
                Value::Null,
                "malformed-request",
                format!("invalid request JSON: {err}"),
            );
        }
    };
    let id = request.id.clone();
    match dispatch(&request) {
        Ok((exit_code, result)) => ok_response(id, exit_code, result),
        Err((code, message)) => error_response(id, code, message),
    }
}

type DispatchError = (&'static str, String);

fn dispatch(request: &Request) -> Result<(u8, Value), DispatchError> {
    match request.method.as_str() {
        "hello" => Ok((crate::cli::EXIT_SUCCESS, hello_result())),
        "check" | "run" | "why" | "whynot" | "audit" | "verify" => {
            dispatch_program_method(&request.method, &request.params)
        }
        "test" => dispatch_test(&request.params),
        m if m.starts_with("kb.") => dispatch_kb(&m["kb.".len()..], &request.params),
        other => Err((
            "unknown-method",
            format!(
                "unknown method '{other}'; supported methods: {}",
                SUPPORTED_METHODS.join(", ")
            ),
        )),
    }
}

fn hello_result() -> Value {
    json!({
        "schema": "brix.serve.hello@1",
        "protocol": SERVE_SCHEMA,
        "toolchain_version": env!("CARGO_PKG_VERSION"),
        "methods": SUPPORTED_METHODS,
    })
}

// ---------------------------------------------------------------------------
// Per-request temporary workspace for inline program/input text
// ---------------------------------------------------------------------------

static WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A private directory created lazily for one request's inline
/// (`source`-given) program/input text, removed on drop. Never created for a
/// request that only names paths.
struct TempWorkspace {
    dir: PathBuf,
}

impl TempWorkspace {
    fn create() -> Result<Self, DispatchError> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let n = WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("brix-serve-{}-{}-{}", std::process::id(), nanos, n));
        std::fs::create_dir_all(&dir).map_err(|e| {
            (
                "workspace-io-error",
                format!("failed to create temporary workspace: {e}"),
            )
        })?;
        Ok(Self { dir })
    }

    fn write(&self, name: &str, contents: &str) -> Result<PathBuf, DispatchError> {
        let path = self.dir.join(name);
        std::fs::write(&path, contents.as_bytes()).map_err(|e| {
            (
                "workspace-io-error",
                format!("failed to write temporary file: {e}"),
            )
        })?;
        Ok(path)
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---------------------------------------------------------------------------
// Source specs: `{"path": "..."}` or `{"source": "..."}`
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct SourceSpec {
    path: Option<String>,
    source: Option<String>,
}

/// Resolve a required program spec to a real filesystem path, materializing
/// `source` (if given) into `workspace` on first use.
fn resolve_program_path(
    workspace: &mut Option<TempWorkspace>,
    spec: &Value,
    field: &str,
) -> Result<PathBuf, DispatchError> {
    let spec: SourceSpec = serde_json::from_value(spec.clone()).map_err(|e| {
        (
            "invalid-params",
            format!("'{field}': expected {{\"path\": ...}} or {{\"source\": ...}}: {e}"),
        )
    })?;
    match (spec.path, spec.source) {
        (Some(_), Some(_)) => Err((
            "invalid-params",
            format!("'{field}': specify exactly one of 'path' or 'source'"),
        )),
        (Some(p), None) => Ok(PathBuf::from(p)),
        (None, Some(src)) => {
            if workspace.is_none() {
                *workspace = Some(TempWorkspace::create()?);
            }
            workspace.as_ref().unwrap().write("program.brix", &src)
        }
        (None, None) => Err((
            "invalid-params",
            format!("'{field}': missing 'path' or 'source'"),
        )),
    }
}

/// Resolve an `inputs` array (each entry a path or inline source spec) to
/// real filesystem paths, materializing inline entries into `workspace`.
fn resolve_input_paths(
    workspace: &mut Option<TempWorkspace>,
    inputs: &Value,
) -> Result<Vec<PathBuf>, DispatchError> {
    if inputs.is_null() {
        return Ok(Vec::new());
    }
    let entries = inputs
        .as_array()
        .ok_or(("invalid-params", "'inputs': expected an array".to_string()))?;
    let mut paths = Vec::with_capacity(entries.len());
    for (i, entry) in entries.iter().enumerate() {
        let spec: SourceSpec = serde_json::from_value(entry.clone()).map_err(|e| {
            (
                "invalid-params",
                format!("'inputs[{i}]': expected {{\"path\": ...}} or {{\"source\": ...}}: {e}"),
            )
        })?;
        let path = match (spec.path, spec.source) {
            (Some(_), Some(_)) => {
                return Err((
                    "invalid-params",
                    format!("'inputs[{i}]': specify exactly one of 'path' or 'source'"),
                ))
            }
            (Some(p), None) => PathBuf::from(p),
            (None, Some(src)) => {
                if workspace.is_none() {
                    *workspace = Some(TempWorkspace::create()?);
                }
                workspace
                    .as_ref()
                    .unwrap()
                    .write(&format!("input_{i}.json"), &src)?
            }
            (None, None) => {
                return Err((
                    "invalid-params",
                    format!("'inputs[{i}]': missing 'path' or 'source'"),
                ))
            }
        };
        paths.push(path);
    }
    Ok(paths)
}

fn string_array_field(params: &Value, field: &str) -> Result<Vec<PathBuf>, DispatchError> {
    match params.get(field) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(i, v)| {
                v.as_str().map(PathBuf::from).ok_or((
                    "invalid-params",
                    format!("'{field}[{i}]': expected a string"),
                ))
            })
            .collect(),
        Some(_) => Err((
            "invalid-params",
            format!("'{field}': expected an array of strings"),
        )),
    }
}

fn required_str_field<'a>(params: &'a Value, field: &str) -> Result<&'a str, DispatchError> {
    params.get(field).and_then(Value::as_str).ok_or((
        "invalid-params",
        format!("missing required string field '{field}'"),
    ))
}

fn optional_bool_field(params: &Value, field: &str) -> bool {
    params.get(field).and_then(Value::as_bool).unwrap_or(false)
}

fn required_u64_field(params: &Value, field: &str) -> Result<u64, DispatchError> {
    params.get(field).and_then(Value::as_u64).ok_or((
        "invalid-params",
        format!("missing or non-integer required field '{field}'"),
    ))
}

fn optional_u64_field(params: &Value, field: &str) -> Option<u64> {
    params.get(field).and_then(Value::as_u64)
}

// ---------------------------------------------------------------------------
// check / run / why / whynot / audit / verify
// ---------------------------------------------------------------------------

fn dispatch_program_method(method: &str, params: &Value) -> Result<(u8, Value), DispatchError> {
    let mut workspace: Option<TempWorkspace> = None;
    let program_spec = params.get("program").cloned().unwrap_or(Value::Null);
    let file = resolve_program_path(&mut workspace, &program_spec, "program")?;
    let package_paths = string_array_field(params, "package_paths")?;
    let input_specs = params.get("inputs").cloned().unwrap_or(Value::Null);
    let input_paths = resolve_input_paths(&mut workspace, &input_specs)?;

    let (result, exit_code) = match method {
        "check" => with_captured_result(|| {
            crate::commands::check::execute_check(&file, true, &package_paths, &input_paths)
        }),
        "run" => with_captured_result(|| {
            crate::commands::run::execute_run(&file, true, &package_paths, &input_paths)
        }),
        "why" | "whynot" => {
            let candidate = required_str_field(params, "candidate")?.to_string();
            let is_whynot = method == "whynot";
            with_captured_result(|| {
                crate::commands::why::execute_why_or_whynot(
                    &file,
                    &candidate,
                    true,
                    &package_paths,
                    &input_paths,
                    is_whynot,
                )
            })
        }
        "audit" => {
            let bundle_out = PathBuf::from(required_str_field(params, "bundle_out")?);
            let force = optional_bool_field(params, "force");
            with_captured_result(|| {
                crate::commands::audit::execute_audit(
                    &file,
                    &bundle_out,
                    force,
                    true,
                    &package_paths,
                    &input_paths,
                )
            })
        }
        "verify" => {
            let bundle = PathBuf::from(required_str_field(params, "bundle")?);
            let expect_program = required_str_field(params, "expect_program")?.to_string();
            let profile = match params.get("profile").and_then(Value::as_str) {
                None | Some("finite-decision") => VerifyProfile::FiniteDecision,
                Some("l3-v1") => VerifyProfile::L3V1,
                Some(other) => {
                    return Err((
                        "invalid-params",
                        format!(
                            "'profile': invalid value '{other}' (expected 'finite-decision' or 'l3-v1')"
                        ),
                    ))
                }
            };
            with_captured_result(|| {
                crate::commands::verify::execute_verify(
                    &expect_program,
                    &file,
                    &bundle,
                    profile,
                    true,
                    &package_paths,
                    &input_paths,
                )
            })
        }
        _ => unreachable!("dispatch_program_method only called for its own method set"),
    };
    Ok((exit_code, result))
}

// ---------------------------------------------------------------------------
// test
// ---------------------------------------------------------------------------

fn dispatch_test(params: &Value) -> Result<(u8, Value), DispatchError> {
    let files = string_array_field(params, "files")?;
    if files.is_empty() {
        return Err((
            "invalid-params",
            "'files': at least one suite file path is required".to_string(),
        ));
    }
    let (result, exit_code) =
        with_captured_result(|| crate::commands::test::execute_test(&files, true));
    Ok((exit_code, result))
}

// ---------------------------------------------------------------------------
// kb.*
// ---------------------------------------------------------------------------

fn dispatch_kb(op_name: &str, params: &Value) -> Result<(u8, Value), DispatchError> {
    let mut workspace: Option<TempWorkspace> = None;
    let dir = PathBuf::from(required_str_field(params, "dir")?);
    let package_paths = string_array_field(params, "package_paths")?;

    let op = match op_name {
        "init" => {
            let program_spec = params.get("program").cloned().unwrap_or(Value::Null);
            let program = resolve_program_path(&mut workspace, &program_spec, "program")?;
            let input_specs = params.get("inputs").cloned().unwrap_or(Value::Null);
            let input_paths = resolve_input_paths(&mut workspace, &input_specs)?;
            KbOp::Init {
                dir,
                program,
                input_paths,
                package_paths,
            }
        }
        "assert" => {
            let input_specs = params.get("inputs").cloned().unwrap_or(Value::Null);
            let input_paths = resolve_input_paths(&mut workspace, &input_specs)?;
            if input_paths.is_empty() {
                return Err((
                    "invalid-params",
                    "'inputs': at least one input is required for kb.assert".to_string(),
                ));
            }
            KbOp::Assert {
                dir,
                input_paths,
                package_paths,
            }
        }
        "retract" => {
            let names = match params.get("names") {
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|v| {
                        v.as_str().map(str::to_string).ok_or((
                            "invalid-params",
                            "'names': expected an array of strings".to_string(),
                        ))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                _ => {
                    return Err((
                        "invalid-params",
                        "'names': expected a non-empty array of strings".to_string(),
                    ))
                }
            };
            if names.is_empty() {
                return Err((
                    "invalid-params",
                    "'names': at least one input name is required for kb.retract".to_string(),
                ));
            }
            KbOp::Retract {
                dir,
                names,
                package_paths,
            }
        }
        "program" => {
            let program_spec = params.get("program").cloned().unwrap_or(Value::Null);
            let program = resolve_program_path(&mut workspace, &program_spec, "program")?;
            KbOp::Program {
                dir,
                program,
                package_paths,
            }
        }
        "log" => KbOp::Log { dir, package_paths },
        "show" => KbOp::Show {
            dir,
            rev: optional_u64_field(params, "rev"),
            package_paths,
        },
        "diff" => KbOp::Diff {
            dir,
            rev_a: required_u64_field(params, "rev_a")?,
            rev_b: required_u64_field(params, "rev_b")?,
            package_paths,
        },
        "audit" => KbOp::Audit {
            dir,
            rev: required_u64_field(params, "rev")?,
            bundle_out: PathBuf::from(required_str_field(params, "bundle_out")?),
            force: optional_bool_field(params, "force"),
            package_paths,
        },
        "verify" => KbOp::Verify { dir, package_paths },
        other => return Err(("unknown-method", format!("unknown method 'kb.{other}'"))),
    };

    let (result, exit_code) = with_captured_result(|| crate::commands::kb::execute_kb(&op, true));
    Ok((exit_code, result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hello_result_shape() {
        let v = hello_result();
        assert_eq!(v["schema"], "brix.serve.hello@1");
        assert_eq!(v["protocol"], SERVE_SCHEMA);
        assert!(v["methods"].as_array().unwrap().contains(&json!("check")));
    }

    #[test]
    fn test_read_bounded_line_basic() {
        let data = b"hello\nworld\n";
        let mut cursor = std::io::BufReader::new(&data[..]);
        match read_bounded_line(&mut cursor, 1024).unwrap() {
            ReadLineResult::Line(s) => assert_eq!(s, "hello"),
            _ => panic!("expected a line"),
        }
        match read_bounded_line(&mut cursor, 1024).unwrap() {
            ReadLineResult::Line(s) => assert_eq!(s, "world"),
            _ => panic!("expected a line"),
        }
        match read_bounded_line(&mut cursor, 1024).unwrap() {
            ReadLineResult::Eof => {}
            _ => panic!("expected eof"),
        }
    }

    #[test]
    fn test_read_bounded_line_too_large() {
        let long_line = "a".repeat(100);
        let data = format!("{long_line}\nshort\n");
        let mut cursor = std::io::BufReader::new(data.as_bytes());
        match read_bounded_line(&mut cursor, 10).unwrap() {
            ReadLineResult::TooLarge => {}
            _ => panic!("expected too-large"),
        }
        match read_bounded_line(&mut cursor, 10).unwrap() {
            ReadLineResult::Line(s) => assert_eq!(s, "short"),
            _ => panic!("expected short line after resync"),
        }
    }

    #[test]
    fn test_read_bounded_line_no_trailing_newline() {
        let data = b"no-newline-here";
        let mut cursor = std::io::BufReader::new(&data[..]);
        match read_bounded_line(&mut cursor, 1024).unwrap() {
            ReadLineResult::Line(s) => assert_eq!(s, "no-newline-here"),
            _ => panic!("expected a line"),
        }
    }

    #[test]
    fn test_dispatch_unknown_method() {
        let req = Request {
            id: json!(1),
            method: "bogus".to_string(),
            params: Value::Null,
        };
        let err = dispatch(&req).unwrap_err();
        assert_eq!(err.0, "unknown-method");
    }

    #[test]
    fn test_handle_line_malformed_json() {
        let response = handle_line("{not json");
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"]["code"], "malformed-request");
    }
}
