//! `brix check` — parse, resolve imports, lower, and preflight check a Brix module.

use std::path::{Path, PathBuf};

use brix_lower::finite_decision::{FiniteDecisionStop, FINITE_DECISION_PROFILE};
use brix_lower::{check_module, evaluate_let_module, render_ty, LetEvalOutcome};
use brix_syntax::ast::Item;

use crate::cli::{EXIT_REJECTED_OR_UNKNOWN, EXIT_SUCCESS, EXIT_USAGE_OR_IO};
use crate::commands::{
    candidate_disposition_to_json, decision_to_json, fact_to_json, fmt_value_human,
    format_finite_decision_human, render_location_snippet, unknown_reason_to_code_and_detail,
};
use crate::json::{to_tagged_value, BindingJson, CliResultJson, LocationJson, BRIX_CLI_SCHEMA};
use crate::pipeline;

/// Execute `brix check <file.brix> [--input <path>...]`.
pub fn execute_check(
    file: &Path,
    json: bool,
    package_paths: &[PathBuf],
    input_paths: &[PathBuf],
) -> u8 {
    let file_display = file.display().to_string();

    let source = match pipeline::stage_read_source("check", file, json) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let parsed = match pipeline::stage_parse("check", &file_display, &source, json) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let resolved_module =
        match pipeline::stage_resolve_imports("check", &parsed.module, package_paths, json) {
            Ok(m) => m,
            Err(code) => return code,
        };

    // Determine if the module is in the finite-decision profile fragment.
    let has_finite_decision_items = resolved_module
        .items
        .iter()
        .any(|i| matches!(i, Item::Commit(_) | Item::Propose(_) | Item::Input(_)));

    if has_finite_decision_items {
        let mut resolved_module = resolved_module;
        crate::commands::prepare_finite_decision_module(&mut resolved_module);
        // Lower as finite-decision. `check` reports the profile it was
        // attempting even on a lowering failure (unlike run/audit/why/whynot,
        // which report `None` there) — a pre-existing difference this
        // consolidation preserves rather than papers over.
        let plan = match pipeline::stage_lower_plan(
            "check",
            Some(FINITE_DECISION_PROFILE.to_string()),
            &file_display,
            &source,
            &parsed.source_map,
            &resolved_module,
            json,
        ) {
            Ok(p) => p,
            Err(code) => return code,
        };
        let program_hex = pipeline::program_id_hex(&plan);

        // If source declares inputs and NO --input was supplied:
        // Validate syntax and lowering/declarations only, do NOT build runtime or run preflight.
        if !plan.inputs.is_empty() && input_paths.is_empty() {
            if json {
                let res = CliResultJson {
                    schema: BRIX_CLI_SCHEMA.to_string(),
                    command: "check".to_string(),
                    ok: true,
                    profile: Some(FINITE_DECISION_PROFILE.to_string()),
                    program: Some(program_hex.clone()),
                    context: None,
                    input_snapshot: None,
                    status: "checked-input-contract".to_string(),
                    inputs: None,
                    facts: Vec::new(),
                    candidates: Vec::new(),
                    decision: None,
                    artifacts: Vec::new(),
                    diagnostics: Vec::new(),
                    bindings: None,
                    explanation: None,
                    locations: None,
                    shows: None,
                    commits: None,
                    entity_decisions: None,
                };
                crate::json::emit_result_json(&res);
            } else {
                println!("status: checked-input-contract");
                println!("program: {program_hex}");
            }
            return EXIT_SUCCESS;
        }

        let profile = Some(FINITE_DECISION_PROFILE.to_string());
        let snapshot = match pipeline::stage_load_snapshot(
            "check",
            profile.clone(),
            Some(program_hex.clone()),
            input_paths,
            json,
        ) {
            Ok(s) => s,
            Err(code) => return code,
        };

        // Preflight: build runtime with inputs and run the plan so check cannot exit success when run would return Unknown.
        let runtime = match pipeline::stage_build_runtime(
            "check",
            profile,
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

        // ADR-0039: preflight fails if *any* commit pool is Unknown, not
        // just the first — every pool must clear preflight independently.
        // ADR-0043: and every `decide` block.
        if let Some(reason) = run.first_fault() {
            let (code, detail) = unknown_reason_to_code_and_detail(reason);
            let status_str = "unknown";
            let diag = format!("{code}: {detail}");
            if json {
                let res = CliResultJson::failure(
                    "check",
                    Some(FINITE_DECISION_PROFILE.to_string()),
                    Some(run.program.0.to_hex()),
                    Some(context_hex),
                    status_str,
                    vec![diag],
                )
                .with_inputs(input_snapshot, inputs_json);
                crate::json::emit_result_json(&res);
            } else {
                eprintln!("brix check: preflight returned Unknown: {diag}");
            }
            return EXIT_REJECTED_OR_UNKNOWN;
        }

        // Preflight succeeded (Selected or Quiescent)
        if json {
            let winning_name = run.decision.as_ref().map(|d| d.candidate.as_str());
            let facts_json = run.facts.iter().map(fact_to_json).collect();
            let candidates_json = run
                .dispositions
                .iter()
                .map(|d| candidate_disposition_to_json(d, winning_name))
                .collect();
            let decision_json = run.decision.as_ref().map(decision_to_json);
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
                command: "check".to_string(),
                ok: true,
                profile: Some(FINITE_DECISION_PROFILE.to_string()),
                program: Some(run.program.0.to_hex()),
                context: Some(context_hex),
                input_snapshot,
                status: "accepted".to_string(),
                inputs: inputs_json,
                facts: facts_json,
                candidates: candidates_json,
                decision: decision_json,
                artifacts: Vec::new(),
                diagnostics: Vec::new(),
                bindings: None,
                explanation: None,
                locations: None,
                shows: None,
                commits: commits_json,
                entity_decisions: None,
            };
            crate::json::emit_result_json(&res);
        } else {
            let human =
                format_finite_decision_human(&run, Some(&context_hex), input_snapshot.as_deref());
            print!("{human}");
        }

        return EXIT_SUCCESS;
    }

    if !input_paths.is_empty() {
        let msg = "--input is only supported for finite-decision modules".to_string();
        if json {
            let res = CliResultJson::failure("check", None, None, None, "usage-error", vec![msg]);
            crate::json::emit_result_json(&res);
        } else {
            eprintln!("brix check: {msg}");
        }
        return EXIT_USAGE_OR_IO;
    }

    // Classic L1/L2 module check path (check_module). `evaluate_let_module`
    // is a separate, additive pass over the same source (ADR-0042): it never
    // changes `check_module`'s type-checking or grades, only adds (or
    // explains the absence of) a value for each binding `check_module`
    // already accepted. The two walk `resolved_module` the same way, so
    // their result vectors line up 1:1 by position.
    let results = check_module(&resolved_module);
    let evaluated = evaluate_let_module(&resolved_module);
    let mut had_error = false;
    let mut diagnostics = Vec::new();
    let mut locations = Vec::new();
    let mut human_lines = Vec::new();
    let mut bindings_json = Vec::new();

    if results.is_empty() {
        human_lines.push("(no `let` bindings to check)".to_string());
    } else {
        for (r, (_, eval_outcome)) in results.iter().zip(evaluated.iter()) {
            match r {
                Ok(cr) => {
                    let ty = cr
                        .ty
                        .as_ref()
                        .map(render_ty)
                        .unwrap_or_else(|| "?".to_string());
                    let grade = format!("{:?}", cr.outcome);
                    match eval_outcome {
                        LetEvalOutcome::Value(v) => {
                            let tagged = to_tagged_value(v);
                            human_lines.push(format!(
                                "  {} : {ty} @{grade} = {}",
                                cr.name,
                                fmt_value_human(v)
                            ));
                            bindings_json
                                .push(BindingJson::evaluated(&cr.name, &ty, &grade, tagged));
                        }
                        LetEvalOutcome::NotEvaluated(reason) => {
                            human_lines.push(format!(
                                "  {} : {ty} @{grade} (not evaluated: {reason})",
                                cr.name
                            ));
                            bindings_json
                                .push(BindingJson::not_evaluated(&cr.name, &ty, &grade, reason));
                        }
                    }
                }
                Err((name, err)) => {
                    had_error = true;
                    let msg = format!("{name}: not checked: {err:?}");
                    diagnostics.push(msg.clone());
                    let mut line = format!("  {msg}");
                    let ident = err
                        .location_subject()
                        .and_then(|(subject, ident)| (subject == name).then_some(ident).flatten());
                    if let Some((loc_line, loc_col)) = parsed.source_map.resolve(name, ident) {
                        if let Some(snippet) =
                            render_location_snippet(&file_display, &source, loc_line, loc_col)
                        {
                            line.push('\n');
                            line.push_str(&snippet);
                        }
                        locations.push(LocationJson::new(&file_display, loc_line, loc_col));
                    }
                    human_lines.push(line);
                }
            }
        }
    }

    if json {
        let res = CliResultJson {
            schema: BRIX_CLI_SCHEMA.to_string(),
            command: "check".to_string(),
            ok: !had_error,
            profile: None,
            program: None,
            context: None,
            input_snapshot: None,
            status: if had_error {
                "rejected".to_string()
            } else {
                "accepted".to_string()
            },
            inputs: None,
            facts: Vec::new(),
            candidates: Vec::new(),
            decision: None,
            artifacts: Vec::new(),
            diagnostics,
            bindings: (!bindings_json.is_empty()).then_some(bindings_json),
            explanation: None,
            locations: (!locations.is_empty()).then_some(locations),
            shows: None,
            commits: None,
            entity_decisions: None,
        };
        crate::json::emit_result_json(&res);
    } else {
        for line in human_lines {
            println!("{line}");
        }
    }

    if had_error {
        EXIT_REJECTED_OR_UNKNOWN
    } else {
        EXIT_SUCCESS
    }
}
