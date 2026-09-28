pub mod audit;
pub mod check;
pub mod explain_render;
pub mod kb;
pub mod run;
pub mod test;
pub mod verify;
pub mod why;

use std::path::PathBuf;

use brix_lower::finite_decision::runtime::{
    BoundInput, CandidateDisposition, DerivedFact, FiniteDecisionRun, FiniteDecisionStop,
    SelectedDecision,
};
use brix_lower::finite_decision::FiniteDecisionBuildError;
use brix_lower::input::{
    load_input_snapshot_from_paths, InputDecodeError, InputError, InputLimits, InputSnapshot,
    InputValidationError,
};
use brix_lower::l3_v2::L3ValueV2;
use soc_regimes::finite_frontier::CandidateStatus;

use crate::json::{
    to_tagged_value, CandidateJson, DecisionJson, EntityDecisionsJson, EntityInstanceJson,
    FactJson, InputJson, StructuredReasonJson,
};

/// Errors encountered when resolving, loading, and validating external inputs for CLI commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliInputError {
    pub code: &'static str,
    pub message: String,
    pub status: &'static str,
    pub exit_code: u8,
}

impl CliInputError {
    pub fn exit_code(&self) -> u8 {
        self.exit_code
    }

    pub fn status(&self) -> &'static str {
        self.status
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn diagnostic(&self) -> String {
        format!("{}: {}", self.code, self.message)
    }

    /// Centralized deterministic human-safe diagnostic renderer for CLI stderr output:
    /// `brix <command>: <status>: <safe_message>`.
    ///
    /// Preserves readable ordinary ASCII, but escapes line breaks, tabs, backslashes as needed,
    /// C0/C1 controls, DEL, and Unicode formatting controls to guarantee structurally single-line
    /// output and prevent terminal escape injection.
    pub fn render_human(&self, cmd: &str) -> String {
        let safe_msg = escape_diagnostic_human(&self.message);
        format!("brix {cmd}: {}: {safe_msg}", self.status)
    }
}

impl From<InputError> for CliInputError {
    fn from(err: InputError) -> Self {
        match err {
            InputError::Decode(InputDecodeError::IoError { path, message }) => Self {
                code: "input-io-error",
                message: format!("failed to read input file '{path}': {message}"),
                status: "io-error",
                exit_code: crate::cli::EXIT_USAGE_OR_IO,
            },
            InputError::Decode(InputDecodeError::NotARegularFile(path)) => Self {
                code: "input-io-error",
                message: format!("input path is not a regular file: {path}"),
                status: "io-error",
                exit_code: crate::cli::EXIT_USAGE_OR_IO,
            },
            InputError::Decode(InputDecodeError::FileTooLarge { limit, found }) => Self {
                code: "input-file-too-large",
                message: format!("input file size ({found} bytes) exceeds limit ({limit} bytes)"),
                status: "rejected",
                exit_code: crate::cli::EXIT_REJECTED_OR_UNKNOWN,
            },
            InputError::Decode(InputDecodeError::DuplicateKey { key, offset }) => Self {
                code: "input-duplicate-key",
                message: format!("duplicate JSON key '{key}' at byte offset {offset}"),
                status: "rejected",
                exit_code: crate::cli::EXIT_REJECTED_OR_UNKNOWN,
            },
            InputError::Decode(InputDecodeError::InvalidSchema { expected, found }) => Self {
                code: "input-schema-mismatch",
                message: format!("schema mismatch: expected '{expected}', found '{found}'"),
                status: "rejected",
                exit_code: crate::cli::EXIT_REJECTED_OR_UNKNOWN,
            },
            InputError::TooManyFiles { limit, found } => Self {
                code: "input-too-many-files",
                message: format!("number of input files ({found}) exceeds limit ({limit})"),
                status: "rejected",
                exit_code: crate::cli::EXIT_REJECTED_OR_UNKNOWN,
            },
            InputError::AggregateBytesExceeded { limit, found } => Self {
                code: "input-aggregate-too-large",
                message: format!(
                    "aggregate input size ({found} bytes) exceeds limit ({limit} bytes)"
                ),
                status: "rejected",
                exit_code: crate::cli::EXIT_REJECTED_OR_UNKNOWN,
            },
            InputError::TotalInputCountExceeded { limit, found } => Self {
                code: "input-count-limit",
                message: format!("total input count ({found}) exceeds limit ({limit})"),
                status: "rejected",
                exit_code: crate::cli::EXIT_REJECTED_OR_UNKNOWN,
            },
            InputError::DuplicateAcrossShards { name } => Self {
                code: "input-duplicate-across-shards",
                message: format!(
                    "duplicate input '{name}' supplied across multiple disjoint shards"
                ),
                status: "rejected",
                exit_code: crate::cli::EXIT_REJECTED_OR_UNKNOWN,
            },
            InputError::Decode(other) => Self {
                code: "input-decode-error",
                message: other.to_string(),
                status: "rejected",
                exit_code: crate::cli::EXIT_REJECTED_OR_UNKNOWN,
            },
        }
    }
}

impl From<InputValidationError> for CliInputError {
    fn from(err: InputValidationError) -> Self {
        match err {
            InputValidationError::MissingInput { name, declared } => Self {
                code: "input-missing",
                message: format!("declared input '{name}' of type {declared} was not supplied"),
                status: "rejected",
                exit_code: crate::cli::EXIT_REJECTED_OR_UNKNOWN,
            },
            InputValidationError::UndeclaredInput { name } => Self {
                code: "input-undeclared",
                message: format!("supplied input '{name}' is not declared in the program"),
                status: "rejected",
                exit_code: crate::cli::EXIT_REJECTED_OR_UNKNOWN,
            },
            InputValidationError::TypeMismatch {
                name,
                declared,
                supplied,
            } => Self {
                code: "input-type-mismatch",
                message: format!(
                    "type mismatch for input '{name}': declared {declared}, supplied {supplied}"
                ),
                status: "rejected",
                exit_code: crate::cli::EXIT_REJECTED_OR_UNKNOWN,
            },
            InputValidationError::InvalidValue { name, detail } => Self {
                code: "input-value-invalid",
                message: format!("invalid value for input '{name}': {detail}"),
                status: "rejected",
                exit_code: crate::cli::EXIT_REJECTED_OR_UNKNOWN,
            },
        }
    }
}

impl From<FiniteDecisionBuildError> for CliInputError {
    fn from(err: FiniteDecisionBuildError) -> Self {
        match err {
            FiniteDecisionBuildError::InputValidation(iv) => Self::from(iv),
            FiniteDecisionBuildError::MissingProposal { candidate } => Self {
                code: "input-decode-error",
                message: format!(
                    "candidate '{candidate}' in commit was not found in declared proposals"
                ),
                status: "rejected",
                exit_code: crate::cli::EXIT_REJECTED_OR_UNKNOWN,
            },
        }
    }
}

/// Centralized loader for external input snapshots in CLI commands (ADR-0031).
///
/// Uses [`load_input_snapshot_from_paths`] with default [`InputLimits`].
/// Distinguishes I/O and file access failures (exit 2, "io-error") from
/// decode, bounds, duplicate key, or schema failures (exit 1, "rejected").
/// Never prints raw file contents in error output.
pub fn load_cli_input_snapshot(input_paths: &[PathBuf]) -> Result<InputSnapshot, CliInputError> {
    load_input_snapshot_from_paths(input_paths, &InputLimits::default())
        .map_err(CliInputError::from)
}

/// Convert a runtime [`BoundInput`] into its canonical JSON representation.
pub fn bound_input_to_json(input: &BoundInput) -> InputJson {
    InputJson::new(
        input.name.clone(),
        to_tagged_value(&input.value),
        input.ordinal,
        "Derived",
    )
}

/// Prepare an AST module for finite-decision lowering by removing surface `show` directives.
pub fn prepare_finite_decision_module(module: &mut brix_syntax::ast::Module) {
    module
        .items
        .retain(|i| !matches!(i, brix_syntax::ast::Item::Show(_)));
}

/// Escape hostile characters in diagnostic messages for safe human terminal display.
///
/// Preserves readable ordinary ASCII (including single and double quotes), but uses standard
/// debug escaping semantics to escape line breaks (`\n`, `\r`), tabs (`\t`), backslashes (`\\`),
/// null (`\0`), C0 and C1 control characters (e.g. `\x1b` -> `\u{1b}`), DEL (`\x7f` -> `\u{7f}`),
/// and Unicode line breaks / formatting / bidi controls (e.g. `\u{2028}`, `\u{202e}`).
pub fn escape_diagnostic_human(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\'' | '"' => out.push(c),
            _ => out.extend(c.escape_debug()),
        }
    }
    out
}

/// Render a rustc-style single-line source snippet with a caret under
/// `(line, column)` (both 1-based), meant to be appended — with a leading
/// newline — after a diagnostic's existing first line:
///
/// ```text
///   --> examples/bad.brix:3:11
///    |
///  3 | propose a(y) priority 1 when x > 0 = A
///    |           ^
/// ```
///
/// The source line is untrusted: every character is escaped through
/// [`escape_diagnostic_human`] before display, and the caret offset is
/// recomputed against the *escaped* text so it still lands under the
/// intended character even when escaping changes the visible width (e.g. a
/// tab becomes the two characters `\t`). A very long line is windowed around
/// the target column — with an ellipsis marking a cut side — so one
/// pathological line cannot dominate terminal output. `column` is a
/// character index (not a byte or terminal-cell index), matching how the
/// lexer counts columns; a wide character still occupies one column here.
///
/// Returns `None` when `line` is out of range for `source` (defensive: a
/// location should always resolve to a real line, but rendering never panics
/// on a mismatch instead).
pub fn render_location_snippet(
    file_display: &str,
    source: &str,
    line: usize,
    column: usize,
) -> Option<String> {
    const MAX_WINDOW_CHARS: usize = 200;
    const CONTEXT_BEFORE: usize = 40;

    if line == 0 {
        return None;
    }
    let raw_line: Vec<char> = source.lines().nth(line - 1)?.chars().collect();
    let col_idx = column.saturating_sub(1).min(raw_line.len());

    let (win_start, win_col_idx, trunc_left) = if raw_line.len() > MAX_WINDOW_CHARS {
        let start = col_idx.saturating_sub(CONTEXT_BEFORE);
        (start, col_idx - start, start > 0)
    } else {
        (0, col_idx, false)
    };
    let win_end = raw_line.len().min(win_start + MAX_WINDOW_CHARS);
    let trunc_right = win_end < raw_line.len();

    let mut text = String::new();
    let mut caret_offset = 0usize;
    for (i, c) in raw_line[win_start..win_end].iter().enumerate() {
        let piece = escape_diagnostic_human(&c.to_string());
        if i < win_col_idx {
            caret_offset += piece.chars().count();
        }
        text.push_str(&piece);
    }
    let left_mark = if trunc_left { "\u{2026} " } else { "" };
    let right_mark = if trunc_right { " \u{2026}" } else { "" };
    caret_offset += left_mark.chars().count();

    let num_str = line.to_string();
    let blank = " ".repeat(num_str.chars().count());
    let caret_spaces = " ".repeat(1 + caret_offset);

    let mut out = String::new();
    out.push(' ');
    out.push_str(&blank);
    out.push_str(&format!("--> {file_display}:{line}:{column}\n"));
    out.push(' ');
    out.push_str(&blank);
    out.push_str(" |\n");
    out.push(' ');
    out.push_str(&num_str);
    out.push_str(&format!(" | {left_mark}{text}{right_mark}\n"));
    out.push(' ');
    out.push_str(&blank);
    out.push_str(" |");
    out.push_str(&caret_spaces);
    out.push('^');
    Some(out)
}

/// Escape a string scalar safely for human rendering.
///
/// Escapes string newlines (`\n`), quotes (`\"`), backslashes (`\\\\`), tabs (`\t`),
/// carriage returns (`\r`), and control characters (e.g. `\u001b`) so that external string
/// values cannot create extra lines or spoof status/program/context fields in human output.
pub fn escape_str_human(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.extend(c.escape_debug()),
        }
    }
    out.push('"');
    out
}

/// Format an [`L3ValueV2`] for human-readable display.
pub fn fmt_value_human(v: &L3ValueV2) -> String {
    match v {
        L3ValueV2::Int(n) => n.to_string(),
        L3ValueV2::Bool(b) => b.to_string(),
        L3ValueV2::Str(s) => escape_str_human(s),
        L3ValueV2::Ctor { variant, args, .. } => {
            if args.is_empty() {
                variant.clone()
            } else {
                let inner = args
                    .iter()
                    .map(fmt_value_human)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{variant}({inner})")
            }
        }
        L3ValueV2::Record {
            nominal_config,
            fields,
        } => {
            let inner = fields
                .iter()
                .map(|(k, val)| format!("{k}: {}", fmt_value_human(val)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{nominal_config} {{ {inner} }}")
        }
        // `[a, b, c]` (ADR-0037, ADR-0040).
        L3ValueV2::List(items) => {
            let inner = items
                .iter()
                .map(fmt_value_human)
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{inner}]")
        }
    }
}

/// Convert a [`DerivedFact`] into its canonical JSON representation.
pub fn fact_to_json(f: &DerivedFact) -> FactJson {
    FactJson {
        name: f.rule.clone(),
        value: to_tagged_value(&f.value),
        ordinal: f.ordinal.to_string(),
        grade: "Derived".to_string(),
    }
}

/// Convert a [`SelectedDecision`] into its canonical JSON representation.
pub fn decision_to_json(d: &SelectedDecision) -> DecisionJson {
    DecisionJson {
        candidate: d.candidate.clone(),
        priority: d.priority.to_string(),
        value: to_tagged_value(&d.value),
        grade: "Derived".to_string(),
    }
}

/// Every commit pool's own outcome (ADR-0039), populated only when the
/// module declares more than one pool, so a single-commit module's JSON is
/// unchanged.
pub fn commits_to_json(
    run: &brix_lower::finite_decision::FiniteDecisionRun,
) -> Option<Vec<crate::json::CommitPoolJson>> {
    if run.commits.len() <= 1 {
        return None;
    }
    Some(
        run.commits
            .iter()
            .map(|c| {
                let win = c.decision.as_ref().map(|d| d.candidate.as_str());
                let status = match &c.stop {
                    FiniteDecisionStop::Selected(_) => "selected",
                    FiniteDecisionStop::Quiescent { .. } => "quiescent",
                    FiniteDecisionStop::Unknown(_) => "unknown",
                };
                crate::json::CommitPoolJson::new(
                    c.commit.clone(),
                    status,
                    c.dispositions
                        .iter()
                        .map(|d| candidate_disposition_to_json(d, win))
                        .collect(),
                    c.decision.as_ref().map(decision_to_json),
                )
            })
            .collect(),
    )
}

/// Every `decide` block's per-entity outcome (ADR-0043), populated only when
/// the module declares at least one block.
pub fn entity_decisions_to_json(
    run: &brix_lower::finite_decision::FiniteDecisionRun,
) -> Option<Vec<EntityDecisionsJson>> {
    if run.decides.is_empty() {
        None
    } else {
        Some(run.decides.iter().map(decide_run_to_json).collect())
    }
}

/// Convert one `decide` block's own run into its JSON representation
/// (ADR-0043).
pub fn decide_run_to_json(
    decide_run: &brix_lower::finite_decision::FiniteDecisionDecideRun,
) -> EntityDecisionsJson {
    use brix_lower::finite_decision::FiniteDecisionDecideStop;

    match &decide_run.stop {
        FiniteDecisionDecideStop::Unknown(reason) => EntityDecisionsJson {
            name: decide_run.decide.clone(),
            status: "unknown".to_string(),
            reason: Some(reason.to_string()),
            instances: Vec::new(),
        },
        FiniteDecisionDecideStop::Settled => EntityDecisionsJson {
            name: decide_run.decide.clone(),
            status: "settled".to_string(),
            reason: None,
            instances: decide_run
                .instances
                .iter()
                .map(|inst| {
                    let winning_name = inst.decision.as_ref().map(|d| d.candidate.as_str());
                    EntityInstanceJson {
                        index: inst.index as u64,
                        binder: to_tagged_value(&inst.binder),
                        status: if inst.decision.is_some() {
                            "selected".to_string()
                        } else {
                            "quiescent".to_string()
                        },
                        candidates: inst
                            .dispositions
                            .iter()
                            .map(|d| candidate_disposition_to_json(d, winning_name))
                            .collect(),
                        decision: inst.decision.as_ref().map(decision_to_json),
                    }
                })
                .collect(),
        },
    }
}

use brix_lower::finite_decision::FiniteDecisionUnknownReason;

/// A run's overall status for reporting and exit codes: `"unknown"` with the
/// first fault if any commit pool or `decide` block failed closed, otherwise
/// the first commit pool's own status (ADR-0039, ADR-0043).
pub fn run_status(
    run: &brix_lower::finite_decision::FiniteDecisionRun,
) -> (&'static str, Option<(&'static str, String)>) {
    if let Some(reason) = run.first_fault() {
        return ("unknown", Some(unknown_reason_to_code_and_detail(reason)));
    }
    match &run.stop {
        FiniteDecisionStop::Selected(_) => ("selected", None),
        FiniteDecisionStop::Quiescent { .. } => ("quiescent", None),
        FiniteDecisionStop::Unknown(reason) => {
            ("unknown", Some(unknown_reason_to_code_and_detail(reason)))
        }
    }
}

/// Map a deliberation unknown reason to a stable reason code and human detail string.
pub fn unknown_reason_to_code_and_detail(
    reason: &FiniteDecisionUnknownReason,
) -> (&'static str, String) {
    let code = match reason {
        FiniteDecisionUnknownReason::ExpressionEvaluationFault { .. } => {
            "expression-evaluation-fault"
        }
        FiniteDecisionUnknownReason::DependencyFault { .. } => "dependency-fault",
        FiniteDecisionUnknownReason::TypeFault { .. } => "type-fault",
        FiniteDecisionUnknownReason::DecisionKeyConflict { .. } => "decision-key-conflict",
        FiniteDecisionUnknownReason::AdmissionError { .. } => "admission-error",
        FiniteDecisionUnknownReason::EvaluationError { .. } => "evaluation-error",
        FiniteDecisionUnknownReason::InvalidPhase { .. } => "invalid-phase",
        FiniteDecisionUnknownReason::QuiescenceVerificationFault { .. } => {
            "quiescence-verification-fault"
        }
        FiniteDecisionUnknownReason::CommitTickError { .. } => "commit-tick-error",
        FiniteDecisionUnknownReason::InvariantViolation { .. } => "invariant-violation",
    };
    (code, reason.to_string())
}

/// Convert a candidate disposition and winning candidate context into [`CandidateJson`].
pub fn candidate_disposition_to_json(
    d: &CandidateDisposition,
    winning_cand: Option<&str>,
) -> CandidateJson {
    let (status, code, detail) = match &d.status {
        CandidateStatus::Selected => (
            "selected".to_string(),
            "selected".to_string(),
            "selected: minimal calendar key".to_string(),
        ),
        CandidateStatus::AdmittedNotSelected => {
            let win = winning_cand.unwrap_or("another candidate");
            (
                "admitted-not-selected".to_string(),
                "overshadowed".to_string(),
                format!("admitted but overshadowed by candidate '{win}'"),
            )
        }
        CandidateStatus::RejectedGuardFalse => (
            "rejected-guard-false".to_string(),
            "guard_false@1".to_string(),
            "guard condition evaluated to false".to_string(),
        ),
        CandidateStatus::Rejected(r) => (
            "rejected".to_string(),
            r.category().to_string(),
            format!("{r}"),
        ),
    };

    CandidateJson {
        name: d.name.clone(),
        priority: d.priority.to_string(),
        status,
        reason: StructuredReasonJson { code, detail },
    }
}

/// Format human output for finite-decision deliberation.
///
/// Disciplinary rule: Never prints Proven or Refuted.
/// Leads with inputs (if present), facts, candidate dispositions, decision or quiescence, and reasons; IDs follow.
pub fn format_finite_decision_human(
    run: &FiniteDecisionRun,
    context_hex: Option<&str>,
    input_snapshot_hex: Option<&str>,
) -> String {
    let mut out = String::new();

    // 0. Inputs (if present)
    if !run.inputs.is_empty() {
        out.push_str("inputs:\n");
        for inp in &run.inputs {
            out.push_str(&format!(
                "  {}: {} @Derived\n",
                inp.name,
                fmt_value_human(&inp.value)
            ));
        }
    }

    // 1. Facts
    if !run.facts.is_empty() {
        out.push_str("facts:\n");
        for f in &run.facts {
            out.push_str(&format!(
                "  {}: {} @Derived\n",
                f.rule,
                fmt_value_human(&f.value)
            ));
        }
    }

    // 2-3. Candidate dispositions, then decision or quiescence — per commit
    // pool (ADR-0039). A single-commit module (still the overwhelming
    // common case) prints exactly the pre-ADR-0039 text: no pool header,
    // just its one pool's own section below.
    // A module made only of `decide` blocks (ADR-0043) has no commit pool,
    // so it prints no top-level decision; its blocks report below.
    match run.commits.len() {
        0 => {}
        1 => push_commit_pool_human(&mut out, &run.commits.first(), &run.dispositions, &run.stop),
        _ => {
            for pool_run in &run.commits {
                out.push_str(&format!("commit {}:\n", pool_run.commit));
                push_commit_pool_human(
                    &mut out,
                    &Some(pool_run),
                    &pool_run.dispositions,
                    &pool_run.stop,
                );
            }
        }
    }

    // 3b. Per-entity decide blocks (ADR-0043), after every commit pool's own
    // section and before IDs — one `decide <name>:` section per block, in
    // declaration order, only when the module declares at least one.
    for decide_run in &run.decides {
        out.push_str(&format!("decide {}:\n", decide_run.decide));
        push_decide_block_human(&mut out, decide_run);
    }

    // 4. IDs follow
    out.push_str(&format!("program: {}\n", run.program.0.to_hex()));
    if let Some(ctx) = context_hex {
        out.push_str(&format!("context: {}\n", ctx));
    }
    if let Some(snap) = input_snapshot_hex {
        out.push_str(&format!("input-snapshot: {}\n", snap));
    }
    for pool_run in &run.commits {
        if let FiniteDecisionStop::Quiescent { certificate } = &pool_run.stop {
            if run.commits.len() > 1 {
                out.push_str(&format!(
                    "certificate ({}): {}\n",
                    pool_run.commit,
                    certificate.digest().to_hex()
                ));
            } else {
                out.push_str(&format!("certificate: {}\n", certificate.digest().to_hex()));
            }
        }
    }

    out
}

/// Render one commit pool's candidate dispositions and its decision or
/// quiescence status into `out`. `pool_run` is `None` only when a shared
/// stage (lets/rules) faulted before any pool was evaluated and the plan
/// somehow has zero commits (never true for a lowered plan; guarded for
/// robustness) — `winning_name`/`stop` are still read straight from the
/// caller's own fields in that case.
fn push_commit_pool_human(
    out: &mut String,
    pool_run: &Option<&brix_lower::finite_decision::FiniteDecisionCommitRun>,
    dispositions: &[CandidateDisposition],
    stop: &FiniteDecisionStop,
) {
    if !dispositions.is_empty() {
        let winning_name = pool_run
            .and_then(|r| r.decision.as_ref())
            .map(|d| d.candidate.as_str());
        out.push_str("candidates:\n");
        for d in dispositions {
            let candidate_json = candidate_disposition_to_json(d, winning_name);
            out.push_str(&format!(
                "  {}: {} (priority {}) — {}\n",
                d.name, candidate_json.status, d.priority, candidate_json.reason.detail
            ));
        }
    }

    match stop {
        FiniteDecisionStop::Selected(sel) => {
            out.push_str(&format!(
                "decision: {} = {} @Derived\nstatus: selected\n",
                sel.candidate,
                fmt_value_human(&sel.value)
            ));
        }
        FiniteDecisionStop::Quiescent { .. } => {
            out.push_str("decision: none (quiescent)\nstatus: quiescent\n");
        }
        FiniteDecisionStop::Unknown(reason) => {
            out.push_str(&format!("status: unknown ({reason})\n"));
        }
    }
}

/// Render one `decide` block's own per-entity outcome (ADR-0043): one line
/// per settled instance in element order, or a single fault line when the
/// whole block is Unknown (all-or-nothing — see
/// `FiniteDecisionDecideRun::stop`'s own docs).
fn push_decide_block_human(
    out: &mut String,
    decide_run: &brix_lower::finite_decision::FiniteDecisionDecideRun,
) {
    use brix_lower::finite_decision::FiniteDecisionDecideStop;

    match &decide_run.stop {
        FiniteDecisionDecideStop::Unknown(reason) => {
            out.push_str(&format!("  status: unknown ({reason})\n"));
        }
        FiniteDecisionDecideStop::Settled => {
            for inst in &decide_run.instances {
                let binder_human = fmt_value_human(&inst.binder);
                match &inst.decision {
                    Some(sel) => {
                        out.push_str(&format!(
                            "  [{}] {}: {} = {} @Derived\n",
                            inst.index,
                            binder_human,
                            sel.candidate,
                            fmt_value_human(&sel.value)
                        ));
                    }
                    None => {
                        out.push_str(&format!(
                            "  [{}] {}: none (quiescent)\n",
                            inst.index, binder_human
                        ));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_str_human_injection_safety() {
        // Plain string
        assert_eq!(escape_str_human("hello"), "\"hello\"");

        // Hostile string with newlines, quotes, backslashes, tabs, carriage returns, and control codes
        let hostile = "status: selected\nprogram: evil\r\t\"with\\quotes\"\x1b[31m\0";
        let escaped = escape_str_human(hostile);

        // Must start and end with quote
        assert!(escaped.starts_with('"'));
        assert!(escaped.ends_with('"'));

        // Must not contain unescaped newline or carriage return
        assert!(!escaped.contains('\n'));
        assert!(!escaped.contains('\r'));
        assert!(!escaped.contains('\t'));
        assert!(!escaped.contains('\x1b'));
        assert!(!escaped.contains('\0'));

        // Must contain escaped representations
        assert!(escaped.contains("\\n"));
        assert!(escaped.contains("\\r"));
        assert!(escaped.contains("\\t"));
        assert!(escaped.contains("\\\""));
        assert!(escaped.contains("\\\\"));
        assert!(escaped.contains("\\u001b"));
        assert!(escaped.contains("\\u0000"));
    }

    #[test]
    fn test_fmt_value_human_nested_injection_safety() {
        let val = L3ValueV2::Ctor {
            variant: "Wrap".to_string(),
            args: vec![L3ValueV2::Str("line1\nline2".to_string())],
            nominal_sum: "Wrap".to_string(),
        };
        let formatted = fmt_value_human(&val);
        assert_eq!(formatted, "Wrap(\"line1\\nline2\")");
        assert!(!formatted.contains('\n'));
    }

    #[test]
    fn test_escape_diagnostic_human_safety() {
        // Plain readable ASCII must be preserved without quotes or mutation
        let plain = "duplicate JSON key 'my_key' at byte offset 42";
        assert_eq!(escape_diagnostic_human(plain), plain);

        // Hostile message with line breaks, tabs, backslashes, C0 controls, DEL, C1 controls, and Unicode formatting
        let hostile = "key: 'start\\slash\nnew\rreturn\ttab\x1b[31mcolor\0null\x7fdel\u{0085}nel\u{2028}ls\u{2029}ps\u{202e}bidi\u{200b}zwsp\u{feff}bom\u{e0001}tag' end";
        let escaped = escape_diagnostic_human(hostile);

        // Must not contain any raw line breaks or dangerous control codes
        assert!(!escaped.contains('\n'));
        assert!(!escaped.contains('\r'));
        assert!(!escaped.contains('\t'));
        assert!(!escaped.contains('\x1b'));
        assert!(!escaped.contains('\0'));
        assert!(!escaped.contains('\x7f'));
        assert!(!escaped.contains('\u{0085}'));
        assert!(!escaped.contains('\u{2028}'));
        assert!(!escaped.contains('\u{2029}'));
        assert!(!escaped.contains('\u{202e}'));
        assert!(!escaped.contains('\u{200b}'));
        assert!(!escaped.contains('\u{feff}'));
        assert!(!escaped.contains('\u{e0001}'));

        // Must contain visible escape sequences using standard Rust debug escaping
        assert!(escaped.contains("\\\\slash"));
        assert!(escaped.contains("\\nnew"));
        assert!(escaped.contains("\\rreturn"));
        assert!(escaped.contains("\\ttab"));
        assert!(escaped.contains("\\u{1b}[31mcolor"));
        assert!(escaped.contains("\\0null"));
        assert!(escaped.contains("\\u{7f}del"));
        assert!(escaped.contains("\\u{85}nel"));
        assert!(escaped.contains("\\u{2028}ls"));
        assert!(escaped.contains("\\u{2029}ps"));
        assert!(escaped.contains("\\u{202e}bidi"));
        assert!(escaped.contains("\\u{200b}zwsp"));
        assert!(escaped.contains("\\u{feff}bom"));
        assert!(escaped.contains("\\u{e0001}tag"));

        // Preserves readable Unicode
        assert_eq!(
            escape_diagnostic_human("message with héllo 世界 🚀"),
            "message with héllo 世界 🚀"
        );
    }

    #[test]
    fn test_cli_input_error_render_human_and_machine_diagnostic() {
        let err = CliInputError {
            code: "input-duplicate-key",
            message: "duplicate JSON key 'bad\nstatus: selected\r\t\x1b[31m' at byte offset 10"
                .to_string(),
            status: "rejected",
            exit_code: 1,
        };

        // Machine diagnostic string must remain unmutated for JSON
        assert_eq!(
            err.diagnostic(),
            "input-duplicate-key: duplicate JSON key 'bad\nstatus: selected\r\t\x1b[31m' at byte offset 10"
        );

        // Human renderer produces single diagnostic line for check/run/audit/why/whynot/verify
        for cmd in &["check", "run", "audit", "why", "whynot", "verify"] {
            let rendered = err.render_human(cmd);
            assert_eq!(
                rendered,
                format!("brix {cmd}: rejected: duplicate JSON key 'bad\\nstatus: selected\\r\\t\\u{{1b}}[31m' at byte offset 10")
            );
            assert_eq!(
                rendered.lines().count(),
                1,
                "rendered output must be structurally 1 line"
            );
        }
    }
}
