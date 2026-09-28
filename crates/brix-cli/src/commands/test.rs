//! `brix test` — regression tests of the form "these inputs → this decision" (ADR-0030, ADR-0031).
//!
//! Reads one or more strict `brix.test@1` JSON suite files. Each suite names a `program`
//! (a `.brix` policy file), optional `package_paths`, and a non-empty list of `cases`. Every
//! case supplies zero or more `--input`-style `brix.input@1`/`brix.input@2` files and an
//! `expect` block asserting the resulting finite-decision deliberation outcome: `status`
//! (required), and any of `decision`, `value`, `unknown_code`, `candidates`, and `facts`.
//!
//! Expected strings are matched against exactly what `brix run` prints in its human-readable
//! output (candidate disposition names such as `rejected-guard-false`, and values rendered by
//! the shared [`crate::commands::fmt_value_human`] formatter, e.g. `Ship` or `"EU-NORTH"`), so a
//! suite author can write expectations by copying straight out of a `brix run` transcript.
//!
//! `program`, `package_paths`, and every case's `inputs` are resolved relative to the test
//! file's own directory (not the current working directory), so a suite is portable regardless
//! of where `brix test` is invoked from.
//!
//! Exit codes: 0 if every case in every named suite passes; 1 if every suite parsed and every
//! program prepared, but at least one case's assertions did not match; 2 if a suite file is
//! missing, oversized, malformed, or empty, or if the program it names cannot be read, parsed,
//! import-resolved, or lowered (nothing can be run in that case, so this is a usage/IO failure,
//! the same category `brix run` uses for a `.brix` file it cannot prepare).

use std::path::{Path, PathBuf};

use serde::Serialize;

use brix_lower::finite_decision::{
    finite_decision_program_id, lower_finite_decision_plan, FiniteDecisionPlan, FiniteDecisionRun,
    FiniteDecisionRuntime, FiniteDecisionStop, FINITE_DECISION_PROFILE,
};
use brix_syntax::parse_bounded;

use crate::cli::{EXIT_REJECTED_OR_UNKNOWN, EXIT_SUCCESS, EXIT_USAGE_OR_IO};
use crate::commands::{
    candidate_disposition_to_json, escape_diagnostic_human, fmt_value_human,
    load_cli_input_snapshot, prepare_finite_decision_module, unknown_reason_to_code_and_detail,
    CliInputError,
};
use crate::packages::{make_package_loader, read_source_bounded};

/// Canonical schema identifier for a `brix test` suite file.
pub const TEST_SCHEMA_V1: &str = "brix.test@1";
/// Canonical schema identifier for the `--json` result object printed by `brix test`.
pub const TEST_RESULT_SCHEMA_V1: &str = "brix.test.result@1";

/// Maximum number of cases accepted in a single suite file.
pub const MAX_TEST_CASES: usize = 1024;
/// Maximum nesting depth accepted by the strict suite-file JSON parser.
const MAX_JSON_DEPTH: usize = 24;
/// Maximum accepted length, in bytes, for a single JSON string value in a suite file.
const MAX_JSON_STRING_BYTES: usize = 65536;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Execute `brix test <file.test.json>... [--json]`.
pub fn execute_test(files: &[PathBuf], json: bool) -> u8 {
    let mut prepared_files = Vec::with_capacity(files.len());
    for path in files {
        match prepare_file(path) {
            Ok(p) => prepared_files.push(p),
            Err(msg) => {
                let full = format!("{}: {msg}", path.display());
                if json {
                    let err_obj = TestFatalErrorJson {
                        schema: TEST_RESULT_SCHEMA_V1.to_string(),
                        ok: false,
                        file: path.display().to_string(),
                        error: msg,
                    };
                    println!("{}", serde_json::to_string_pretty(&err_obj).unwrap());
                } else {
                    eprintln!("brix test: {}", escape_diagnostic_human(&full));
                }
                return EXIT_USAGE_OR_IO;
            }
        }
    }

    let mut file_results: Vec<FileRunResult<'_>> = Vec::with_capacity(prepared_files.len());
    let mut total_passed = 0usize;
    let mut total_failed = 0usize;

    for prepared in &prepared_files {
        let mut cases = Vec::with_capacity(prepared.spec.cases.len());
        let mut passed = 0usize;
        let mut failed = 0usize;
        for case in &prepared.spec.cases {
            let outcome = run_case(prepared, case);
            if outcome.ok {
                passed += 1;
            } else {
                failed += 1;
            }
            cases.push(outcome);
        }
        total_passed += passed;
        total_failed += failed;
        file_results.push(FileRunResult {
            prepared,
            cases,
            passed,
            failed,
        });
    }

    if json {
        print_json_results(&file_results, total_passed, total_failed);
    } else {
        print_human_results(&file_results, total_passed, total_failed);
    }

    if total_failed == 0 {
        EXIT_SUCCESS
    } else {
        EXIT_REJECTED_OR_UNKNOWN
    }
}

struct FileRunResult<'a> {
    prepared: &'a PreparedFile,
    cases: Vec<CaseOutcome>,
    passed: usize,
    failed: usize,
}

// ---------------------------------------------------------------------------
// Preparing a suite file and its program
// ---------------------------------------------------------------------------

struct PreparedFile {
    display_path: String,
    base_dir: PathBuf,
    spec: TestFileSpec,
    plan: FiniteDecisionPlan,
}

/// Read, strictly parse, and prepare a single `.test.json` suite file and the program it names.
///
/// Any failure here (missing/oversized/malformed suite file, or a program that cannot be read,
/// parsed, import-resolved, or lowered) is fatal to the whole invocation: without a working
/// program, no case in the file can be evaluated, so this is reported the same way `brix run`
/// reports a `.brix` file it cannot prepare — a usage/IO failure (exit 2), not a failing case.
fn prepare_file(path: &Path) -> Result<PreparedFile, String> {
    let raw = read_source_bounded(path)?;
    let spec = parse_test_file(&raw).map_err(|e| e.to_string())?;

    let base_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(""));
    let program_path = base_dir.join(&spec.program);
    let package_paths: Vec<PathBuf> = spec
        .package_paths
        .iter()
        .map(|p| base_dir.join(p))
        .collect();

    let source = read_source_bounded(&program_path)
        .map_err(|e| format!("cannot prepare program '{}': {e}", spec.program))?;
    let module = parse_bounded(&source, brix_syntax::ParseLimits::strict()).map_err(|e| {
        format!(
            "cannot prepare program '{}': parse error: {e}",
            spec.program
        )
    })?;
    let loader = make_package_loader(&package_paths);
    let mut resolved_module =
        brix_lower::imports::resolve_imports(&module, &loader).map_err(|e| {
            format!(
                "cannot prepare program '{}': import error: {e:?}",
                spec.program
            )
        })?;
    prepare_finite_decision_module(&mut resolved_module);
    let plan =
        lower_finite_decision_plan(&resolved_module, FINITE_DECISION_PROFILE).map_err(|e| {
            format!(
                "cannot prepare program '{}': lowering error: {e}",
                spec.program
            )
        })?;

    Ok(PreparedFile {
        display_path: path.display().to_string(),
        base_dir,
        spec,
        plan,
    })
}

// ---------------------------------------------------------------------------
// Running and comparing a single case
// ---------------------------------------------------------------------------

struct Mismatch {
    field: String,
    expected: String,
    actual: String,
}

impl Mismatch {
    fn new(
        field: impl Into<String>,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Self {
        Self {
            field: field.into(),
            expected: expected.into(),
            actual: actual.into(),
        }
    }
}

struct CaseOutcome {
    name: String,
    ok: bool,
    program_id_hex: Option<String>,
    input_snapshot_hex: Option<String>,
    resolved_inputs: Vec<String>,
    /// Set instead of `mismatches` when the case could not be evaluated at all (e.g. an
    /// `--input`-equivalent file failed to load, or declared/supplied inputs disagreed).
    error: Option<String>,
    mismatches: Vec<Mismatch>,
}

fn run_case(prepared: &PreparedFile, case: &TestCaseSpec) -> CaseOutcome {
    let resolved_inputs: Vec<PathBuf> = case
        .inputs
        .iter()
        .map(|p| prepared.base_dir.join(p))
        .collect();
    let resolved_display: Vec<String> = resolved_inputs
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    let program_id_hex = Some(finite_decision_program_id(&prepared.plan).0.to_hex());

    let snapshot = match load_cli_input_snapshot(&resolved_inputs) {
        Ok(s) => s,
        Err(err) => {
            return CaseOutcome {
                name: case.name.clone(),
                ok: false,
                program_id_hex,
                input_snapshot_hex: None,
                resolved_inputs: resolved_display,
                error: Some(err.diagnostic()),
                mismatches: Vec::new(),
            };
        }
    };
    let input_snapshot_hex = if !snapshot.is_empty() {
        Some(snapshot.id().0.to_hex())
    } else {
        None
    };

    let runtime = match FiniteDecisionRuntime::build_with_inputs(&prepared.plan, &snapshot) {
        Ok(r) => r,
        Err(err) => {
            let cli_err = CliInputError::from(err);
            return CaseOutcome {
                name: case.name.clone(),
                ok: false,
                program_id_hex,
                input_snapshot_hex,
                resolved_inputs: resolved_display,
                error: Some(cli_err.diagnostic()),
                mismatches: Vec::new(),
            };
        }
    };

    let run = runtime.run();
    let mismatches = compare_case(&case.expect, &run);
    CaseOutcome {
        name: case.name.clone(),
        ok: mismatches.is_empty(),
        program_id_hex: Some(run.program.0.to_hex()),
        input_snapshot_hex,
        resolved_inputs: resolved_display,
        error: None,
        mismatches,
    }
}

/// Compare a case's `expect` block against an actual [`FiniteDecisionRun`], reusing exactly the
/// same status/candidate/value rendering `brix run` uses so expectations read like transcripts.
fn compare_case(expect: &ExpectSpec, run: &FiniteDecisionRun) -> Vec<Mismatch> {
    let mut mismatches = Vec::new();
    let winning_name = run.decision.as_ref().map(|d| d.candidate.as_str());

    let actual_status = match &run.stop {
        FiniteDecisionStop::Selected(_) => "selected",
        FiniteDecisionStop::Quiescent { .. } => "quiescent",
        FiniteDecisionStop::Unknown(_) => "unknown",
    };
    if actual_status != expect.status.as_str() {
        mismatches.push(Mismatch::new(
            "status",
            expect.status.as_str(),
            actual_status,
        ));
    }

    if let Some(expected_decision) = &expect.decision {
        let actual_decision = run
            .decision
            .as_ref()
            .map(|d| d.candidate.clone())
            .unwrap_or_else(|| "(none)".to_string());
        if &actual_decision != expected_decision {
            mismatches.push(Mismatch::new(
                "decision",
                expected_decision.clone(),
                actual_decision,
            ));
        }
    }

    if let Some(expected_value) = &expect.value {
        let actual_value = run
            .decision
            .as_ref()
            .map(|d| fmt_value_human(&d.value))
            .unwrap_or_else(|| "(none)".to_string());
        if &actual_value != expected_value {
            mismatches.push(Mismatch::new("value", expected_value.clone(), actual_value));
        }
    }

    if let Some(expected_code) = &expect.unknown_code {
        let actual_code = match &run.stop {
            FiniteDecisionStop::Unknown(reason) => {
                unknown_reason_to_code_and_detail(reason).0.to_string()
            }
            _ => "(not unknown)".to_string(),
        };
        if &actual_code != expected_code {
            mismatches.push(Mismatch::new(
                "unknown_code",
                expected_code.clone(),
                actual_code,
            ));
        }
    }

    for (name, expected_status) in &expect.candidates {
        let field = format!("candidates.{name}");
        match run.dispositions.iter().find(|d| &d.name == name) {
            Some(d) => {
                let cj = candidate_disposition_to_json(d, winning_name);
                if &cj.status != expected_status {
                    mismatches.push(Mismatch::new(field, expected_status.clone(), cj.status));
                }
            }
            None => mismatches.push(Mismatch::new(
                field,
                expected_status.clone(),
                "(no such candidate)",
            )),
        }
    }

    for (name, expected_value) in &expect.facts {
        let field = format!("facts.{name}");
        match run.facts.iter().find(|f| &f.rule == name) {
            Some(f) => {
                let actual = fmt_value_human(&f.value);
                if &actual != expected_value {
                    mismatches.push(Mismatch::new(field, expected_value.clone(), actual));
                }
            }
            None => mismatches.push(Mismatch::new(
                field,
                expected_value.clone(),
                "(no such fact)",
            )),
        }
    }

    mismatches
}

// ---------------------------------------------------------------------------
// Human output
// ---------------------------------------------------------------------------

fn print_human_results(
    file_results: &[FileRunResult<'_>],
    total_passed: usize,
    total_failed: usize,
) {
    let multi = file_results.len() > 1;
    for (idx, file_result) in file_results.iter().enumerate() {
        if multi {
            if idx > 0 {
                println!();
            }
            println!(
                "{}:",
                escape_diagnostic_human(&file_result.prepared.display_path)
            );
        }
        for outcome in &file_result.cases {
            print_case_human(outcome);
        }
    }
    println!("test result: {total_passed} passed, {total_failed} failed");
}

fn print_case_human(outcome: &CaseOutcome) {
    let safe_name = escape_diagnostic_human(&outcome.name);
    if outcome.ok {
        println!("ok   {safe_name}");
        return;
    }
    println!("FAIL {safe_name}");
    if let Some(err) = &outcome.error {
        println!("  error: {}", escape_diagnostic_human(err));
        return;
    }
    for m in &outcome.mismatches {
        println!(
            "  {}: expected: {}, actual: {}",
            escape_diagnostic_human(&m.field),
            escape_diagnostic_human(&m.expected),
            escape_diagnostic_human(&m.actual)
        );
    }
}

// ---------------------------------------------------------------------------
// JSON output (`brix.test.result@1`)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct TestFatalErrorJson {
    schema: String,
    ok: bool,
    file: String,
    error: String,
}

#[derive(Serialize)]
struct TestSuiteResultJson {
    schema: String,
    ok: bool,
    passed: usize,
    failed: usize,
    files: Vec<TestFileResultJson>,
}

#[derive(Serialize)]
struct TestFileResultJson {
    file: String,
    program: String,
    passed: usize,
    failed: usize,
    cases: Vec<TestCaseResultJson>,
}

#[derive(Serialize)]
struct TestCaseResultJson {
    name: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    program: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_snapshot: Option<String>,
    inputs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    mismatches: Vec<MismatchJson>,
}

#[derive(Serialize)]
struct MismatchJson {
    field: String,
    expected: String,
    actual: String,
}

fn print_json_results(
    file_results: &[FileRunResult<'_>],
    total_passed: usize,
    total_failed: usize,
) {
    let files = file_results
        .iter()
        .map(|file_result| TestFileResultJson {
            file: file_result.prepared.display_path.clone(),
            program: file_result.prepared.spec.program.clone(),
            passed: file_result.passed,
            failed: file_result.failed,
            cases: file_result.cases.iter().map(case_outcome_to_json).collect(),
        })
        .collect();

    let res = TestSuiteResultJson {
        schema: TEST_RESULT_SCHEMA_V1.to_string(),
        ok: total_failed == 0,
        passed: total_passed,
        failed: total_failed,
        files,
    };
    println!("{}", serde_json::to_string_pretty(&res).unwrap());
}

fn case_outcome_to_json(outcome: &CaseOutcome) -> TestCaseResultJson {
    TestCaseResultJson {
        name: outcome.name.clone(),
        ok: outcome.ok,
        program: outcome.program_id_hex.clone(),
        input_snapshot: outcome.input_snapshot_hex.clone(),
        inputs: outcome.resolved_inputs.clone(),
        error: outcome.error.clone(),
        mismatches: outcome
            .mismatches
            .iter()
            .map(|m| MismatchJson {
                field: m.field.clone(),
                expected: m.expected.clone(),
                actual: m.actual.clone(),
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Suite file schema (`brix.test@1`) and its strict, bounded, duplicate-key-rejecting parser
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct TestFileSpec {
    program: String,
    package_paths: Vec<String>,
    cases: Vec<TestCaseSpec>,
}

#[derive(Debug)]
struct TestCaseSpec {
    name: String,
    inputs: Vec<String>,
    expect: ExpectSpec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedStatus {
    Selected,
    Quiescent,
    Unknown,
}

impl ExpectedStatus {
    fn as_str(self) -> &'static str {
        match self {
            ExpectedStatus::Selected => "selected",
            ExpectedStatus::Quiescent => "quiescent",
            ExpectedStatus::Unknown => "unknown",
        }
    }
}

#[derive(Debug)]
struct ExpectSpec {
    status: ExpectedStatus,
    decision: Option<String>,
    value: Option<String>,
    unknown_code: Option<String>,
    candidates: Vec<(String, String)>,
    facts: Vec<(String, String)>,
}

/// A parse/validation failure for a `.test.json` suite file. Always fatal (exit 2): see
/// [`prepare_file`].
#[derive(Debug, Clone)]
struct TestFileError(String);

impl TestFileError {
    fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl std::fmt::Display for TestFileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn parse_test_file(raw: &str) -> Result<TestFileSpec, TestFileError> {
    let root = JsonParser::new(raw).parse_root()?;
    let entries = as_object(&root, "test file root")?;

    let mut schema: Option<String> = None;
    let mut program: Option<String> = None;
    let mut package_paths: Vec<String> = Vec::new();
    let mut cases: Option<Vec<TestCaseSpec>> = None;

    for (key, value) in entries {
        match key.as_str() {
            "schema" => schema = Some(as_string(value, "'schema'")?),
            "program" => program = Some(as_string(value, "'program'")?),
            "package_paths" => {
                let items = as_array(value, "'package_paths'")?;
                for item in items {
                    package_paths.push(as_string(item, "'package_paths[]'")?);
                }
            }
            "cases" => {
                let items = as_array(value, "'cases'")?;
                if items.is_empty() {
                    return Err(TestFileError::new(
                        "'cases' must not be empty: a test file must declare at least one case",
                    ));
                }
                if items.len() > MAX_TEST_CASES {
                    return Err(TestFileError::new(format!(
                        "'cases' has {} entries, exceeding the limit of {MAX_TEST_CASES}",
                        items.len()
                    )));
                }
                let mut parsed = Vec::with_capacity(items.len());
                for item in items {
                    parsed.push(parse_case(item)?);
                }
                cases = Some(parsed);
            }
            other => {
                return Err(TestFileError::new(format!(
                    "unknown field '{other}' in test file root"
                )))
            }
        }
    }

    let schema = schema.ok_or_else(|| TestFileError::new("missing required field 'schema'"))?;
    if schema != TEST_SCHEMA_V1 {
        return Err(TestFileError::new(format!(
            "unsupported schema '{schema}': expected '{TEST_SCHEMA_V1}'"
        )));
    }
    let program = program.ok_or_else(|| TestFileError::new("missing required field 'program'"))?;
    let cases = cases.ok_or_else(|| TestFileError::new("missing required field 'cases'"))?;

    Ok(TestFileSpec {
        program,
        package_paths,
        cases,
    })
}

fn parse_case(v: &JsonVal) -> Result<TestCaseSpec, TestFileError> {
    let entries = as_object(v, "case entry")?;
    let mut name: Option<String> = None;
    let mut inputs: Vec<String> = Vec::new();
    let mut expect: Option<ExpectSpec> = None;

    for (key, value) in entries {
        match key.as_str() {
            "name" => name = Some(as_string(value, "case 'name'")?),
            "inputs" => {
                let items = as_array(value, "case 'inputs'")?;
                for item in items {
                    inputs.push(as_string(item, "case 'inputs[]'")?);
                }
            }
            "expect" => expect = Some(parse_expect(value)?),
            other => {
                return Err(TestFileError::new(format!(
                    "unknown field '{other}' in test case"
                )))
            }
        }
    }

    let name = name.ok_or_else(|| TestFileError::new("test case missing required field 'name'"))?;
    let expect = expect.ok_or_else(|| {
        TestFileError::new(format!(
            "test case '{name}' missing required field 'expect'"
        ))
    })?;

    Ok(TestCaseSpec {
        name,
        inputs,
        expect,
    })
}

fn parse_expect(v: &JsonVal) -> Result<ExpectSpec, TestFileError> {
    let entries = as_object(v, "'expect'")?;
    let mut status: Option<ExpectedStatus> = None;
    let mut decision: Option<String> = None;
    let mut value: Option<String> = None;
    let mut unknown_code: Option<String> = None;
    let mut candidates: Vec<(String, String)> = Vec::new();
    let mut facts: Vec<(String, String)> = Vec::new();

    for (key, val) in entries {
        match key.as_str() {
            "status" => {
                let s = as_string(val, "'expect.status'")?;
                status = Some(match s.as_str() {
                    "selected" => ExpectedStatus::Selected,
                    "quiescent" => ExpectedStatus::Quiescent,
                    "unknown" => ExpectedStatus::Unknown,
                    other => {
                        return Err(TestFileError::new(format!(
                            "invalid 'expect.status' value '{other}': expected 'selected', 'quiescent', or 'unknown'"
                        )))
                    }
                });
            }
            "decision" => decision = Some(as_string(val, "'expect.decision'")?),
            "value" => value = Some(as_string(val, "'expect.value'")?),
            "unknown_code" => unknown_code = Some(as_string(val, "'expect.unknown_code'")?),
            "candidates" => {
                let obj = as_object(val, "'expect.candidates'")?;
                for (name, status_val) in obj {
                    candidates.push((
                        name.clone(),
                        as_string(status_val, "'expect.candidates[]'")?,
                    ));
                }
            }
            "facts" => {
                let obj = as_object(val, "'expect.facts'")?;
                for (name, fact_val) in obj {
                    facts.push((name.clone(), as_string(fact_val, "'expect.facts[]'")?));
                }
            }
            other => {
                return Err(TestFileError::new(format!(
                    "unknown field '{other}' in 'expect'"
                )))
            }
        }
    }

    let status =
        status.ok_or_else(|| TestFileError::new("'expect' missing required field 'status'"))?;

    Ok(ExpectSpec {
        status,
        decision,
        value,
        unknown_code,
        candidates,
        facts,
    })
}

// ---------------------------------------------------------------------------
// Minimal strict, bounded, duplicate-key-rejecting JSON value parser.
//
// `brix-lower`'s `input.rs` already implements this discipline for the `brix.input@N`
// envelope, but its parser is private and shaped around that envelope's specific tagged-value
// grammar. The `brix.test@1` schema is a plain tree of objects/arrays/strings, so a small,
// self-contained parser is implemented here rather than exposing or generalizing that one.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum JsonVal {
    Null,
    Bool,
    Number,
    Str(String),
    Array(Vec<JsonVal>),
    /// Preserves source order; construction rejects duplicate keys within one object.
    Object(Vec<(String, JsonVal)>),
}

impl JsonVal {
    fn type_name(&self) -> &'static str {
        match self {
            JsonVal::Null => "null",
            JsonVal::Bool => "bool",
            JsonVal::Number => "number",
            JsonVal::Str(_) => "string",
            JsonVal::Array(_) => "array",
            JsonVal::Object(_) => "object",
        }
    }
}

fn as_object<'a>(v: &'a JsonVal, ctx: &str) -> Result<&'a [(String, JsonVal)], TestFileError> {
    match v {
        JsonVal::Object(entries) => Ok(entries),
        other => Err(TestFileError::new(format!(
            "expected object for {ctx}, found {}",
            other.type_name()
        ))),
    }
}

fn as_string(v: &JsonVal, ctx: &str) -> Result<String, TestFileError> {
    match v {
        JsonVal::Str(s) => Ok(s.clone()),
        other => Err(TestFileError::new(format!(
            "expected string for {ctx}, found {}",
            other.type_name()
        ))),
    }
}

fn as_array<'a>(v: &'a JsonVal, ctx: &str) -> Result<&'a [JsonVal], TestFileError> {
    match v {
        JsonVal::Array(items) => Ok(items),
        other => Err(TestFileError::new(format!(
            "expected array for {ctx}, found {}",
            other.type_name()
        ))),
    }
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    pos: usize,
    depth: usize,
}

impl<'a> JsonParser<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            text,
            pos: 0,
            depth: 0,
        }
    }

    fn err_here(&self, message: impl Into<String>) -> TestFileError {
        TestFileError::new(format!("{} at byte offset {}", message.into(), self.pos))
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.bytes.get(self.pos) {
            match b {
                b' ' | b'\t' | b'\r' | b'\n' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn enter_depth(&mut self) -> Result<(), TestFileError> {
        if self.depth >= MAX_JSON_DEPTH {
            return Err(self.err_here("JSON nesting depth limit exceeded"));
        }
        self.depth += 1;
        Ok(())
    }

    fn leave_depth(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn parse_root(&mut self) -> Result<JsonVal, TestFileError> {
        self.skip_ws();
        let v = self.parse_value()?;
        self.skip_ws();
        if self.pos != self.bytes.len() {
            return Err(self.err_here("trailing data after JSON document"));
        }
        Ok(v)
    }

    fn parse_value(&mut self) -> Result<JsonVal, TestFileError> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b'"') => Ok(JsonVal::Str(self.parse_string()?)),
            Some(b't') => {
                self.consume_lit(b"true")?;
                Ok(JsonVal::Bool)
            }
            Some(b'f') => {
                self.consume_lit(b"false")?;
                Ok(JsonVal::Bool)
            }
            Some(b'n') => {
                self.consume_lit(b"null")?;
                Ok(JsonVal::Null)
            }
            Some(b'-') | Some(b'0'..=b'9') => {
                self.parse_number()?;
                Ok(JsonVal::Number)
            }
            Some(other) => Err(self.err_here(format!("unexpected character '{}'", other as char))),
            None => Err(self.err_here("unexpected end of input")),
        }
    }

    fn consume_lit(&mut self, lit: &[u8]) -> Result<(), TestFileError> {
        if self.bytes[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            Ok(())
        } else {
            Err(self.err_here(format!(
                "expected literal '{}'",
                String::from_utf8_lossy(lit)
            )))
        }
    }

    fn parse_object(&mut self) -> Result<JsonVal, TestFileError> {
        self.enter_depth()?;
        self.pos += 1; // consume '{'
        let mut entries: Vec<(String, JsonVal)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            self.leave_depth();
            return Ok(JsonVal::Object(entries));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.err_here("expected string key in object"));
            }
            let key_offset = self.pos;
            let key = self.parse_string()?;
            if entries.iter().any(|(k, _)| k == &key) {
                return Err(TestFileError::new(format!(
                    "duplicate JSON key '{key}' at byte offset {key_offset}"
                )));
            }
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(self.err_here("expected ':' after object key"));
            }
            self.pos += 1;
            let value = self.parse_value()?;
            entries.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(self.err_here("expected ',' or '}' in object")),
            }
        }
        self.leave_depth();
        Ok(JsonVal::Object(entries))
    }

    fn parse_array(&mut self) -> Result<JsonVal, TestFileError> {
        self.enter_depth()?;
        self.pos += 1; // consume '['
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            self.leave_depth();
            return Ok(JsonVal::Array(items));
        }
        loop {
            let value = self.parse_value()?;
            items.push(value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(self.err_here("expected ',' or ']' in array")),
            }
        }
        self.leave_depth();
        Ok(JsonVal::Array(items))
    }

    fn parse_string(&mut self) -> Result<String, TestFileError> {
        // Precondition: self.peek() == Some(b'"').
        self.pos += 1;
        let mut s = String::new();
        loop {
            let b = *self
                .bytes
                .get(self.pos)
                .ok_or_else(|| TestFileError::new("unterminated string literal"))?;
            if b == b'"' {
                self.pos += 1;
                return Ok(s);
            } else if b == b'\\' {
                self.pos += 1;
                let esc = *self
                    .bytes
                    .get(self.pos)
                    .ok_or_else(|| TestFileError::new("unterminated escape sequence"))?;
                self.pos += 1;
                match esc {
                    b'"' => s.push('"'),
                    b'\\' => s.push('\\'),
                    b'/' => s.push('/'),
                    b'b' => s.push('\u{8}'),
                    b'f' => s.push('\u{c}'),
                    b'n' => s.push('\n'),
                    b'r' => s.push('\r'),
                    b't' => s.push('\t'),
                    b'u' => s.push(self.parse_unicode_escape()?),
                    other => {
                        return Err(self
                            .err_here(format!("invalid escape character '\\{}'", other as char)))
                    }
                }
            } else if b < 0x20 {
                return Err(self.err_here("control character in string literal"));
            } else {
                let ch = self.text[self.pos..].chars().next().unwrap();
                s.push(ch);
                self.pos += ch.len_utf8();
            }
            if s.len() > MAX_JSON_STRING_BYTES {
                return Err(TestFileError::new(format!(
                    "string value exceeds maximum length ({MAX_JSON_STRING_BYTES} bytes)"
                )));
            }
        }
    }

    fn parse_unicode_escape(&mut self) -> Result<char, TestFileError> {
        let code = self.read_hex4()?;
        if (0xD800..=0xDBFF).contains(&code) {
            if self.bytes.get(self.pos) != Some(&b'\\')
                || self.bytes.get(self.pos + 1) != Some(&b'u')
            {
                return Err(self.err_here("lone high surrogate in \\u escape"));
            }
            self.pos += 2;
            let low = self.read_hex4()?;
            if !(0xDC00..=0xDFFF).contains(&low) {
                return Err(self.err_here("invalid low surrogate in \\u escape"));
            }
            let scalar = 0x10000 + (((code - 0xD800) as u32) << 10) + (low - 0xDC00) as u32;
            char::from_u32(scalar).ok_or_else(|| self.err_here("invalid unicode scalar"))
        } else if (0xDC00..=0xDFFF).contains(&code) {
            Err(self.err_here("lone low surrogate in \\u escape"))
        } else {
            char::from_u32(code as u32).ok_or_else(|| self.err_here("invalid unicode scalar"))
        }
    }

    fn read_hex4(&mut self) -> Result<u16, TestFileError> {
        if self.pos + 4 > self.bytes.len() {
            return Err(self.err_here("truncated \\u escape"));
        }
        let mut val: u16 = 0;
        for _ in 0..4 {
            let b = self.bytes[self.pos];
            let digit = match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                b'A'..=b'F' => b - b'A' + 10,
                _ => return Err(self.err_here("invalid hex digit in \\u escape")),
            };
            val = (val << 4) | (digit as u16);
            self.pos += 1;
        }
        Ok(val)
    }

    fn parse_number(&mut self) -> Result<String, TestFileError> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
            }
            Some(b'1'..=b'9') => {
                self.pos += 1;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.pos += 1;
                }
            }
            _ => return Err(self.err_here("invalid number literal")),
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err_here("invalid number literal"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.err_here("invalid number literal"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        Ok(self.text[start..self.pos].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> TestFileSpec {
        parse_test_file(src).expect("expected test file to parse")
    }

    fn parse_err(src: &str) -> String {
        parse_test_file(src).unwrap_err().to_string()
    }

    #[test]
    fn test_parses_minimal_valid_suite() {
        let spec = parse_ok(
            r#"{
                "schema": "brix.test@1",
                "program": "p.brix",
                "cases": [
                    {"name": "c1", "inputs": [], "expect": {"status": "quiescent"}}
                ]
            }"#,
        );
        assert_eq!(spec.program, "p.brix");
        assert!(spec.package_paths.is_empty());
        assert_eq!(spec.cases.len(), 1);
        assert_eq!(spec.cases[0].name, "c1");
        assert!(spec.cases[0].expect.status == ExpectedStatus::Quiescent);
    }

    #[test]
    fn test_parses_full_expect_block() {
        let spec = parse_ok(
            r#"{
                "schema": "brix.test@1",
                "program": "p.brix",
                "package_paths": ["../pkgs"],
                "cases": [
                    {
                        "name": "ships",
                        "inputs": ["in.json"],
                        "expect": {
                            "status": "selected",
                            "decision": "ship",
                            "value": "Ship",
                            "candidates": {"hold": "admitted-not-selected"},
                            "facts": {"threshold": "10"}
                        }
                    }
                ]
            }"#,
        );
        assert_eq!(spec.package_paths, vec!["../pkgs".to_string()]);
        let case = &spec.cases[0];
        assert_eq!(case.inputs, vec!["in.json".to_string()]);
        assert_eq!(case.expect.decision.as_deref(), Some("ship"));
        assert_eq!(case.expect.value.as_deref(), Some("Ship"));
        assert_eq!(
            case.expect.candidates,
            vec![("hold".to_string(), "admitted-not-selected".to_string())]
        );
        assert_eq!(
            case.expect.facts,
            vec![("threshold".to_string(), "10".to_string())]
        );
    }

    #[test]
    fn test_rejects_unknown_top_level_field() {
        let err = parse_err(
            r#"{"schema": "brix.test@1", "program": "p.brix", "cases": [{"name":"c","expect":{"status":"quiescent"}}], "bogus": 1}"#,
        );
        assert!(err.contains("unknown field 'bogus'"), "{err}");
    }

    #[test]
    fn test_rejects_unknown_expect_field() {
        let err = parse_err(
            r#"{"schema": "brix.test@1", "program": "p.brix", "cases": [{"name":"c","expect":{"status":"quiescent","bogus":"x"}}]}"#,
        );
        assert!(err.contains("unknown field 'bogus' in 'expect'"), "{err}");
    }

    #[test]
    fn test_rejects_duplicate_key_at_root() {
        let err = parse_err(
            r#"{"schema": "brix.test@1", "schema": "brix.test@1", "program": "p.brix", "cases": [{"name":"c","expect":{"status":"quiescent"}}]}"#,
        );
        assert!(err.contains("duplicate JSON key 'schema'"), "{err}");
    }

    #[test]
    fn test_rejects_duplicate_key_in_candidates() {
        let err = parse_err(
            r#"{"schema": "brix.test@1", "program": "p.brix", "cases": [{"name":"c","expect":{"status":"selected","candidates":{"a":"selected","a":"rejected"}}}]}"#,
        );
        assert!(err.contains("duplicate JSON key 'a'"), "{err}");
    }

    #[test]
    fn test_rejects_empty_cases() {
        let err = parse_err(r#"{"schema": "brix.test@1", "program": "p.brix", "cases": []}"#);
        assert!(err.contains("must not be empty"), "{err}");
    }

    #[test]
    fn test_rejects_missing_status() {
        let err = parse_err(
            r#"{"schema": "brix.test@1", "program": "p.brix", "cases": [{"name":"c","expect":{}}]}"#,
        );
        assert!(err.contains("missing required field 'status'"), "{err}");
    }

    #[test]
    fn test_rejects_invalid_status_value() {
        let err = parse_err(
            r#"{"schema": "brix.test@1", "program": "p.brix", "cases": [{"name":"c","expect":{"status":"maybe"}}]}"#,
        );
        assert!(
            err.contains("invalid 'expect.status' value 'maybe'"),
            "{err}"
        );
    }

    #[test]
    fn test_rejects_wrong_schema() {
        let err = parse_err(
            r#"{"schema": "brix.test@2", "program": "p.brix", "cases": [{"name":"c","expect":{"status":"quiescent"}}]}"#,
        );
        assert!(err.contains("unsupported schema 'brix.test@2'"), "{err}");
    }

    #[test]
    fn test_rejects_trailing_data() {
        let err = parse_err(r#"{"schema": "brix.test@1", "program": "p.brix", "cases": []} extra"#);
        assert!(err.contains("trailing data"), "{err}");
    }

    #[test]
    fn test_rejects_too_many_cases() {
        let mut cases = String::new();
        for i in 0..(MAX_TEST_CASES + 1) {
            if i > 0 {
                cases.push(',');
            }
            cases.push_str(&format!(
                r#"{{"name":"c{i}","expect":{{"status":"quiescent"}}}}"#
            ));
        }
        let src =
            format!(r#"{{"schema": "brix.test@1", "program": "p.brix", "cases": [{cases}]}}"#);
        let err = parse_err(&src);
        assert!(err.contains("exceeding the limit"), "{err}");
    }

    #[test]
    fn test_compare_case_status_mismatch() {
        // A synthetic mismatch check without running the full pipeline: verifies field naming.
        let m = Mismatch::new("status", "selected", "quiescent");
        assert_eq!(m.field, "status");
        assert_eq!(m.expected, "selected");
        assert_eq!(m.actual, "quiescent");
    }
}
