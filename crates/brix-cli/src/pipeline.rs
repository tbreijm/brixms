//! The shared CLI front-end pipeline: read source → parse → resolve imports →
//! lower → load input snapshot → build the finite-decision runtime. (Callers
//! strip `show` items — via `commands::prepare_finite_decision_module` —
//! between resolving imports and lowering; that step is not itself part of
//! this pipeline because `run` alone needs the pre-strip module too, to
//! evaluate `show` separately — see `commands::run`'s module doc.)
//!
//! `check`/`run`/`audit`/`why`/`whynot` each need this exact sequence (ADR-0030),
//! previously re-implemented independently in each `commands::*` module with
//! copy-pasted human/JSON failure emission. This module factors it into one
//! set of stage functions plus [`emit_failure`], the single failure-emission
//! helper: each stage does its own work and, on failure, calls `emit_failure`
//! and returns the CLI exit code as `Err`, so a caller writes
//! `let x = pipeline::stage_foo(...).map_err(|code| return code)?;`-shaped
//! code (via early `return`) instead of duplicating the printing.
//!
//! `verify` shares only the source-read stage and the input-snapshot stage
//! (see their doc comments) — its bundle-decode/expect-program flow and its
//! "unknown" (not "rejected") status convention for a source mismatch are
//! different enough that forcing it through the rest of this pipeline would
//! only obscure both.
//!
//! Every stage is behaviour-preserving: same JSON fields/values, same human
//! text, same exit codes as the pre-consolidation per-command code.

use std::path::{Path, PathBuf};

use brix_lower::finite_decision::{
    finite_decision_program_id, lower_finite_decision_plan, FiniteDecisionPlan,
    FiniteDecisionRuntime, FINITE_DECISION_PROFILE,
};
use brix_lower::imports::resolve_imports;
use brix_lower::input::InputSnapshot;
use brix_syntax::ast::Module;
use brix_syntax::{parse_bounded_with_source_map, ParseLimits, SourceMap};

use crate::cli::{EXIT_REJECTED_OR_UNKNOWN, EXIT_USAGE_OR_IO};
use crate::commands::CliInputError;
use crate::json::CliResultJson;
use crate::packages::{make_package_loader, read_source_bounded};

/// Print `payload` as pretty JSON, or `human_line` to stderr — the one place
/// every pipeline-stage failure (and nothing else) is rendered.
pub fn emit_failure(json: bool, payload: CliResultJson, human_line: &str) {
    if json {
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
    } else {
        eprintln!("{human_line}");
    }
}

/// Stage: read source text under the strict size bound (ADR-0022).
///
/// On failure: JSON status `"io-error"`, human `brix {cmd}: {err}` (the
/// message from [`read_source_bounded`] is already a complete sentence).
pub fn stage_read_source(cmd: &str, file: &Path, json: bool) -> Result<String, u8> {
    match read_source_bounded(file) {
        Ok(s) => Ok(s),
        Err(err) => {
            let payload =
                CliResultJson::failure(cmd, None, None, None, "io-error", vec![err.clone()]);
            emit_failure(json, payload, &format!("brix {cmd}: {err}"));
            Err(EXIT_USAGE_OR_IO)
        }
    }
}

/// The result of a successful parse-with-location stage: the module and its
/// sidecar source map (see [`brix_syntax::source_map`]), kept together since
/// every lowering-error diagnostic downstream needs both.
pub struct ParsedSource {
    pub module: Module,
    pub source_map: SourceMap,
}

/// Stage: parse source into a module (plus its sidecar source map) under
/// strict limits.
///
/// On failure: JSON status `"rejected"` with diagnostic `"parse error: {err}"`
/// (unchanged) plus an additive `locations` entry when the parser reported a
/// line/column; human `brix {cmd}: rejected: {msg}` with a rustc-style
/// snippet appended when a location is available.
pub fn stage_parse(
    cmd: &str,
    file_display: &str,
    source: &str,
    json: bool,
) -> Result<ParsedSource, u8> {
    match parse_bounded_with_source_map(source, ParseLimits::strict()) {
        Ok((module, source_map)) => Ok(ParsedSource { module, source_map }),
        Err(err) => {
            let msg = format!("parse error: {err}");
            let mut human = format!("brix {cmd}: rejected: {msg}");
            let mut locations = None;
            if let (Some(line), Some(col)) = (err.line, err.col) {
                if let Some(snippet) =
                    crate::commands::render_location_snippet(file_display, source, line, col)
                {
                    human.push('\n');
                    human.push_str(&snippet);
                }
                locations = Some(vec![crate::json::LocationJson::new(
                    file_display,
                    line,
                    col,
                )]);
            }
            let payload = CliResultJson::failure(cmd, None, None, None, "rejected", vec![msg])
                .with_locations(locations);
            emit_failure(json, payload, &human);
            Err(EXIT_REJECTED_OR_UNKNOWN)
        }
    }
}

/// Stage: resolve `use` imports against configured package search paths.
///
/// On failure: JSON status `"rejected"` with diagnostic
/// `"import error: {err}"` (now [`std::fmt::Display`], not `{err:?}`); human
/// `brix {cmd}: rejected: {msg}`.
pub fn stage_resolve_imports(
    cmd: &str,
    module: &Module,
    package_paths: &[PathBuf],
    json: bool,
) -> Result<Module, u8> {
    let loader = make_package_loader(package_paths);
    match resolve_imports(module, &loader) {
        Ok(m) => Ok(m),
        Err(err) => {
            let msg = format!("import error: {err}");
            let payload =
                CliResultJson::failure(cmd, None, None, None, "rejected", vec![msg.clone()]);
            emit_failure(json, payload, &format!("brix {cmd}: rejected: {msg}"));
            Err(EXIT_REJECTED_OR_UNKNOWN)
        }
    }
}

/// Stage: lower a (`show`-stripped) resolved module into a finite-decision
/// plan.
///
/// `profile_on_error` is the `profile` JSON field to report if lowering
/// fails: existing commands disagree here (`check` reports the profile it was
/// attempting; `run`/`audit`/`why`/`whynot` report `None`), so callers supply
/// their own pre-existing value rather than this stage picking one.
///
/// On failure: JSON status `"rejected"` with diagnostic
/// `"lowering error: {err}"` plus an additive `locations` entry when
/// `source_map` resolves the error's [`FiniteDecisionLowerError::location_subject`];
/// human likewise with a rustc-style snippet appended when available.
pub fn stage_lower_plan(
    cmd: &str,
    profile_on_error: Option<String>,
    file_display: &str,
    source: &str,
    source_map: &SourceMap,
    module: &Module,
    json: bool,
) -> Result<FiniteDecisionPlan, u8> {
    match lower_finite_decision_plan(module, FINITE_DECISION_PROFILE) {
        Ok(p) => Ok(p),
        Err(err) => {
            let msg = format!("lowering error: {err}");
            let mut human = format!("brix {cmd}: rejected: {msg}");
            let mut locations = None;
            if let Some((item, ident)) = err.location_subject() {
                if let Some((line, col)) = source_map.resolve(item, ident) {
                    if let Some(snippet) =
                        crate::commands::render_location_snippet(file_display, source, line, col)
                    {
                        human.push('\n');
                        human.push_str(&snippet);
                    }
                    locations = Some(vec![crate::json::LocationJson::new(
                        file_display,
                        line,
                        col,
                    )]);
                }
            }
            let payload =
                CliResultJson::failure(cmd, profile_on_error, None, None, "rejected", vec![msg])
                    .with_locations(locations);
            emit_failure(json, payload, &human);
            Err(EXIT_REJECTED_OR_UNKNOWN)
        }
    }
}

/// Stage: load the external input snapshot from `--input` paths.
///
/// `profile`/`program` are attached to the failure JSON exactly as the caller
/// supplies them (callers that have already lowered a plan pass its profile
/// and program id; `verify` passes its externally pinned program hex).
///
/// On failure: JSON status/diagnostic from [`CliInputError`]; human via
/// [`CliInputError::render_human`].
pub fn stage_load_snapshot(
    cmd: &str,
    profile: Option<String>,
    program: Option<String>,
    input_paths: &[PathBuf],
    json: bool,
) -> Result<InputSnapshot, u8> {
    match crate::commands::load_cli_input_snapshot(input_paths) {
        Ok(s) => Ok(s),
        Err(err) => {
            let payload = CliResultJson::failure(
                cmd,
                profile,
                program,
                None,
                err.status(),
                vec![err.diagnostic()],
            );
            emit_failure(json, payload, &err.render_human(cmd));
            Err(err.exit_code())
        }
    }
}

/// Stage: build the finite-decision runtime against the plan and bound
/// inputs.
///
/// On failure: JSON status/diagnostic from [`CliInputError`] (wrapping the
/// build error) with `input_snapshot` attached when the snapshot is
/// nonempty; human via [`CliInputError::render_human`].
pub fn stage_build_runtime(
    cmd: &str,
    profile: Option<String>,
    program: Option<String>,
    plan: &FiniteDecisionPlan,
    snapshot: &InputSnapshot,
    json: bool,
) -> Result<FiniteDecisionRuntime, u8> {
    match FiniteDecisionRuntime::build_with_inputs(plan, snapshot) {
        Ok(r) => Ok(r),
        Err(err) => {
            let cli_err = CliInputError::from(err);
            let snapshot_hex = if !snapshot.is_empty() {
                Some(snapshot.id().0.to_hex())
            } else {
                None
            };
            let payload = CliResultJson::failure(
                cmd,
                profile,
                program,
                None,
                cli_err.status(),
                vec![cli_err.diagnostic()],
            )
            .with_inputs(snapshot_hex, None);
            emit_failure(json, payload, &cli_err.render_human(cmd));
            Err(cli_err.exit_code())
        }
    }
}

/// The plan's program id as lowercase hex, computed once and threaded through
/// the later stages that need it in JSON failures.
pub fn program_id_hex(plan: &FiniteDecisionPlan) -> String {
    finite_decision_program_id(plan).0.to_hex()
}
