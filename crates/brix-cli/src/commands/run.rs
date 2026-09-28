//! `brix run` — finite-decision deliberation execution (ADR-0030).

use std::path::{Path, PathBuf};

use brix_lower::finite_decision::{
    lower_finite_decision_plan, FiniteDecisionRuntime, FiniteDecisionStop, FINITE_DECISION_PROFILE,
};
use brix_syntax::ast::{Expr, Item};

use crate::cli::{EXIT_REJECTED_OR_UNKNOWN, EXIT_SUCCESS};
use crate::commands::{
    candidate_disposition_to_json, decision_to_json, fact_to_json, fmt_value_human,
    format_finite_decision_human, unknown_reason_to_code_and_detail,
};
use crate::json::{to_tagged_value, CliResultJson, TaggedValue, BRIX_CLI_SCHEMA};
use crate::pipeline;

/// The label and evaluated value of one `show` expression, ready for
/// human/JSON rendering.
struct ShowResult {
    label: String,
    value: TaggedValue,
    human_value: String,
}

/// The outcome of attempting to evaluate a module's `show` expressions.
enum ShowsOutcome {
    /// No `show` items were declared: nothing to print.
    None,
    /// Every `show` expression evaluated without fault.
    Values(Vec<ShowResult>),
    /// Lowering or evaluating a `show` expression faulted. Carries a
    /// diagnostic string; the already-committed decision is unaffected.
    Fault(String),
}

/// The label under which a `show` expression's value is printed: its bare
/// variable name when it is exactly `show <name>` (the common case — showing
/// a rule fact or the commit's own committed value), else a positional
/// fallback for a more general expression.
fn show_label(expr: &Expr, idx: usize) -> String {
    match expr {
        Expr::Var(name) => name.clone(),
        _ => format!("show[{idx}]"),
    }
}

/// Lower and evaluate every `show` item in `module_with_shows` against the
/// already-built `runtime`/`run` (see `FiniteDecisionLowerError`'s and
/// `FiniteDecisionRuntime::evaluate_shows_exprs`'s doc comments for why this
/// is a second, separate lowering rather than reusing the identity-bearing
/// `plan`).
fn evaluate_module_shows(
    module_with_shows: &brix_syntax::ast::Module,
    runtime: &FiniteDecisionRuntime,
    run: &brix_lower::finite_decision::FiniteDecisionRun,
) -> ShowsOutcome {
    let labels: Vec<String> = module_with_shows
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Show(expr) => Some(expr),
            _ => None,
        })
        .enumerate()
        .map(|(idx, expr)| show_label(expr, idx))
        .collect();
    if labels.is_empty() {
        return ShowsOutcome::None;
    }

    let shows_plan = match lower_finite_decision_plan(module_with_shows, FINITE_DECISION_PROFILE) {
        Ok(p) => p,
        Err(err) => return ShowsOutcome::Fault(format!("lowering error: {err}")),
    };

    match runtime.evaluate_shows_exprs(run, &shows_plan.shows) {
        Ok(values) => {
            let results = labels
                .into_iter()
                .zip(values)
                .map(|(label, v)| ShowResult {
                    label,
                    value: to_tagged_value(&v),
                    human_value: fmt_value_human(&v),
                })
                .collect();
            ShowsOutcome::Values(results)
        }
        Err(reason) => {
            let (code, detail) = unknown_reason_to_code_and_detail(&reason);
            ShowsOutcome::Fault(format!("{code}: {detail}"))
        }
    }
}

/// Execute `brix run <file.brix> [--input <path>...]`.
pub fn execute_run(
    file: &Path,
    json: bool,
    package_paths: &[PathBuf],
    input_paths: &[PathBuf],
) -> u8 {
    let file_display = file.display().to_string();

    let source = match pipeline::stage_read_source("run", file, json) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let parsed = match pipeline::stage_parse("run", &file_display, &source, json) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let resolved_module =
        match pipeline::stage_resolve_imports("run", &parsed.module, package_paths, json) {
            Ok(m) => m,
            Err(code) => return code,
        };

    // Kept *before* `show` items are stripped, so declared `show` expressions
    // can be lowered/evaluated separately without ever feeding the plan whose
    // canonical program identity is computed below (see `evaluate_module_shows`).
    let module_with_shows = resolved_module.clone();

    let mut show_free_module = resolved_module;
    crate::commands::prepare_finite_decision_module(&mut show_free_module);

    let plan = match pipeline::stage_lower_plan(
        "run",
        None,
        &file_display,
        &source,
        &parsed.source_map,
        &show_free_module,
        json,
    ) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let program_hex = pipeline::program_id_hex(&plan);
    let profile = Some(FINITE_DECISION_PROFILE.to_string());

    let snapshot = match pipeline::stage_load_snapshot(
        "run",
        profile.clone(),
        Some(program_hex.clone()),
        input_paths,
        json,
    ) {
        Ok(s) => s,
        Err(code) => return code,
    };

    let runtime = match pipeline::stage_build_runtime(
        "run",
        profile.clone(),
        Some(program_hex),
        &plan,
        &snapshot,
        json,
    ) {
        Ok(r) => r,
        Err(code) => return code,
    };
    let context_hex = runtime.context.digest().to_hex();
    let run = runtime.run();

    let is_ok = !run.is_unknown();
    let (status_str, mut diagnostics) = match &run.stop {
        FiniteDecisionStop::Selected(_) => ("selected".to_string(), Vec::new()),
        FiniteDecisionStop::Quiescent { .. } => ("quiescent".to_string(), Vec::new()),
        FiniteDecisionStop::Unknown(reason) => {
            let (code, detail) = unknown_reason_to_code_and_detail(reason);
            ("unknown".to_string(), vec![format!("{code}: {detail}")])
        }
    };

    let (input_snapshot, inputs_json) = if !snapshot.is_empty() {
        (
            Some(snapshot.id().0.to_hex()),
            Some(
                run.inputs
                    .iter()
                    .map(crate::commands::bound_input_to_json)
                    .collect(),
            ),
        )
    } else {
        (None, None)
    };

    let mut human =
        format_finite_decision_human(&run, Some(&context_hex), input_snapshot.as_deref());

    // `show` results, evaluated after the decision so a fault there can never
    // alter what was already committed (ADR-0030): the decision above is
    // final regardless of what follows.
    let mut ok = is_ok;
    let mut shows_json: Option<Vec<TaggedValue>> = None;
    match evaluate_module_shows(&module_with_shows, &runtime, &run) {
        ShowsOutcome::None => {}
        ShowsOutcome::Values(results) => {
            if !results.is_empty() {
                human.push_str("shows:\n");
                for r in &results {
                    human.push_str(&format!("  {} = {} @Derived\n", r.label, r.human_value));
                }
            }
            shows_json = Some(results.into_iter().map(|r| r.value).collect());
        }
        ShowsOutcome::Fault(detail) => {
            human.push_str(&format!("shows: unknown ({detail})\n"));
            diagnostics.push(format!("show-fault: {detail}"));
            ok = false;
        }
    }

    if json {
        let winning_name = run.decision.as_ref().map(|d| d.candidate.as_str());
        let facts_json = run.facts.iter().map(fact_to_json).collect();
        let candidates_json = run
            .dispositions
            .iter()
            .map(|d| candidate_disposition_to_json(d, winning_name))
            .collect();
        let decision_json = run.decision.as_ref().map(decision_to_json);
        // ADR-0039: every commit pool's own outcome, additive — populated
        // only when the module declares more than one pool, so a
        // single-commit module's JSON is unchanged (see the field's doc).
        let commits_json = if run.commits.len() > 1 {
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
        } else {
            None
        };

        let res = CliResultJson {
            schema: BRIX_CLI_SCHEMA.to_string(),
            command: "run".to_string(),
            ok,
            profile: Some(FINITE_DECISION_PROFILE.to_string()),
            program: Some(run.program.0.to_hex()),
            context: Some(context_hex),
            input_snapshot,
            status: status_str,
            inputs: inputs_json,
            facts: facts_json,
            candidates: candidates_json,
            decision: decision_json,
            artifacts: Vec::new(),
            diagnostics,
            explanation: None,
            locations: None,
            shows: shows_json,
            commits: commits_json,
        };
        println!("{}", serde_json::to_string_pretty(&res).unwrap());
    } else {
        print!("{human}");
    }

    if ok {
        EXIT_SUCCESS
    } else {
        EXIT_REJECTED_OR_UNKNOWN
    }
}
