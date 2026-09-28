//! `brix why` and `brix whynot` — deliberation explanation re-derivation (ADR-0030).

use std::path::{Path, PathBuf};

use brix_lower::finite_decision::FINITE_DECISION_PROFILE;
use soc_regimes::finite_frontier::{WhyExplanation, WhyNotExplanation};

use crate::cli::{EXIT_REJECTED_OR_UNKNOWN, EXIT_SUCCESS, EXIT_USAGE_OR_IO};
use crate::commands::{
    candidate_disposition_to_json, decision_to_json, fact_to_json, format_finite_decision_human,
    unknown_reason_to_code_and_detail,
};
use crate::json::{CliResultJson, BRIX_CLI_SCHEMA};
use crate::pipeline;

/// Execute `brix why` or `brix whynot`.
#[allow(clippy::too_many_arguments)]
pub fn execute_why_or_whynot(
    file: &Path,
    candidate: &str,
    entity: Option<usize>,
    json: bool,
    package_paths: &[PathBuf],
    input_paths: &[PathBuf],
    is_whynot: bool,
) -> u8 {
    let cmd_name = if is_whynot { "whynot" } else { "why" };
    let file_display = file.display().to_string();

    let source = match pipeline::stage_read_source(cmd_name, file, json) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let parsed = match pipeline::stage_parse(cmd_name, &file_display, &source, json) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let resolved_module =
        match pipeline::stage_resolve_imports(cmd_name, &parsed.module, package_paths, json) {
            Ok(m) => m,
            Err(code) => return code,
        };

    let mut resolved_module = resolved_module;
    crate::commands::prepare_finite_decision_module(&mut resolved_module);

    let plan = match pipeline::stage_lower_plan(
        cmd_name,
        None,
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
    let profile = Some(FINITE_DECISION_PROFILE.to_string());

    let snapshot = match pipeline::stage_load_snapshot(
        cmd_name,
        profile.clone(),
        Some(program_hex.clone()),
        input_paths,
        json,
    ) {
        Ok(s) => s,
        Err(code) => return code,
    };

    let runtime = match pipeline::stage_build_runtime(
        cmd_name,
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

    // A `decide`-scoped candidate (ADR-0043) needs one more coordinate — an
    // element index — since its name was declared once but instantiated
    // once per element; `--entity INDEX` supplies it. Handled entirely
    // separately from the commit-pool path below: `--entity` is required
    // for a decide-owned candidate and refused for anything else.
    if let Some(decide) = plan.decide_of_candidate(candidate) {
        return explain_entity(
            cmd_name,
            decide,
            candidate,
            entity,
            &run,
            &context_hex,
            input_snapshot,
            inputs_json,
            json,
            is_whynot,
        );
    }
    if entity.is_some() {
        let msg = format!(
            "'--entity' is only meaningful for a candidate declared inside a 'decide' block; '{candidate}' is not one"
        );
        if json {
            let res = CliResultJson::failure(
                cmd_name,
                Some(FINITE_DECISION_PROFILE.to_string()),
                Some(run.program.0.to_hex()),
                Some(context_hex.clone()),
                "usage-error",
                vec![msg],
            )
            .with_inputs(input_snapshot, inputs_json);
            crate::json::emit_result_json(&res);
        } else {
            eprintln!("brix {cmd_name}: {msg}");
        }
        return EXIT_USAGE_OR_IO;
    }

    // Fail-closed pre-flight, scoped to `candidate`'s own commit pool
    // (ADR-0039) — a fault in a different pool must not block explaining a
    // candidate in a healthy one. Falls back to the first pool's stop when
    // `candidate` names no candidate in any pool (matching pre-ADR-0039
    // behavior on a single-commit plan, and `explain_why`/`explain_why_not`
    // below still report `CandidateNotFound` in that case).
    let relevant_stop = match plan.commit_of_candidate(candidate) {
        Some(pool) => run
            .commit_run(&pool.name)
            .map(|c| c.stop.clone())
            .unwrap_or_else(|| run.stop.clone()),
        None => run.stop.clone(),
    };
    if let brix_lower::finite_decision::FiniteDecisionStop::Unknown(reason) = &relevant_stop {
        let (code, detail) = unknown_reason_to_code_and_detail(reason);
        let status_str = "unknown";
        let diag = format!("{code}: {detail}");
        if json {
            let res = CliResultJson::failure(
                cmd_name,
                Some(FINITE_DECISION_PROFILE.to_string()),
                Some(run.program.0.to_hex()),
                Some(context_hex),
                status_str,
                vec![diag],
            )
            .with_inputs(input_snapshot, inputs_json);
            crate::json::emit_result_json(&res);
        } else {
            eprintln!("brix {cmd_name}: deliberation resulted in Unknown: {diag}");
        }
        return EXIT_REJECTED_OR_UNKNOWN;
    }

    let explanation_text = if !is_whynot {
        match runtime.explain_why(candidate) {
            Ok(WhyExplanation::Selected { candidate, .. }) => {
                format!(
                    "{}: selected — admitted with minimal calendar key",
                    candidate.name
                )
            }
            Ok(WhyExplanation::AdmittedNotSelected {
                candidate,
                selected_candidate,
                ..
            }) => {
                format!(
                    "{}: admitted-not-selected — overshadowed by selected candidate '{}'",
                    candidate.name, selected_candidate.name
                )
            }
            Ok(WhyExplanation::NotAdmitted { candidate, reason }) => {
                format!("{}: not-admitted — rejected ({reason})", candidate.name)
            }
            Ok(WhyExplanation::CandidateNotFound) => {
                let msg = format!("candidate '{candidate}' not found in candidate pool");
                if json {
                    let res = CliResultJson::failure(
                        cmd_name,
                        Some(FINITE_DECISION_PROFILE.to_string()),
                        Some(run.program.0.to_hex()),
                        Some(context_hex),
                        "candidate-not-found",
                        vec![msg],
                    )
                    .with_inputs(input_snapshot, inputs_json);
                    crate::json::emit_result_json(&res);
                } else {
                    eprintln!("brix {cmd_name}: {msg}");
                }
                return EXIT_REJECTED_OR_UNKNOWN;
            }
            Ok(WhyExplanation::EvaluationFaulted { fault, .. }) => {
                let msg = format!("deliberation fault: {fault}");
                if json {
                    let res = CliResultJson::failure(
                        cmd_name,
                        Some(FINITE_DECISION_PROFILE.to_string()),
                        Some(run.program.0.to_hex()),
                        Some(context_hex),
                        "unknown",
                        vec![msg],
                    )
                    .with_inputs(input_snapshot, inputs_json);
                    crate::json::emit_result_json(&res);
                } else {
                    eprintln!("brix {cmd_name}: {msg}");
                }
                return EXIT_REJECTED_OR_UNKNOWN;
            }
            Ok(WhyExplanation::NoCandidateAdmitted) => {
                format!("{candidate}: no candidate admitted (certified quiescence)")
            }
            Err(reason) => {
                let msg = format!("{reason}");
                if json {
                    let res = CliResultJson::failure(
                        cmd_name,
                        Some(FINITE_DECISION_PROFILE.to_string()),
                        Some(run.program.0.to_hex()),
                        Some(context_hex),
                        "unknown",
                        vec![msg],
                    )
                    .with_inputs(input_snapshot, inputs_json);
                    crate::json::emit_result_json(&res);
                } else {
                    eprintln!("brix {cmd_name}: {msg}");
                }
                return EXIT_REJECTED_OR_UNKNOWN;
            }
        }
    } else {
        match runtime.explain_why_not(candidate) {
            Ok(WhyNotExplanation::RejectedByPolicy { candidate, reason }) => {
                format!("{}: rejected — {reason}", candidate.name)
            }
            Ok(WhyNotExplanation::Overshadowed {
                candidate,
                selected_candidate,
                ..
            }) => {
                format!(
                    "{}: overshadowed — admitted but lost selection to higher-priority candidate '{}'",
                    candidate.name, selected_candidate.name
                )
            }
            Ok(WhyNotExplanation::ActuallySelected { candidate, .. }) => {
                format!(
                    "{}: actually-selected — candidate was admitted and selected",
                    candidate.name
                )
            }
            Ok(WhyNotExplanation::CandidateNotFound) => {
                let msg = format!("candidate '{candidate}' not found in candidate pool");
                if json {
                    let res = CliResultJson::failure(
                        cmd_name,
                        Some(FINITE_DECISION_PROFILE.to_string()),
                        Some(run.program.0.to_hex()),
                        Some(context_hex),
                        "candidate-not-found",
                        vec![msg],
                    )
                    .with_inputs(input_snapshot, inputs_json);
                    crate::json::emit_result_json(&res);
                } else {
                    eprintln!("brix {cmd_name}: {msg}");
                }
                return EXIT_REJECTED_OR_UNKNOWN;
            }
            Ok(WhyNotExplanation::EvaluationFaulted { fault, .. }) => {
                let msg = format!("deliberation fault: {fault}");
                if json {
                    let res = CliResultJson::failure(
                        cmd_name,
                        Some(FINITE_DECISION_PROFILE.to_string()),
                        Some(run.program.0.to_hex()),
                        Some(context_hex),
                        "unknown",
                        vec![msg],
                    )
                    .with_inputs(input_snapshot, inputs_json);
                    crate::json::emit_result_json(&res);
                } else {
                    eprintln!("brix {cmd_name}: {msg}");
                }
                return EXIT_REJECTED_OR_UNKNOWN;
            }
            Ok(WhyNotExplanation::NoCandidateSelected) => {
                format!("{candidate}: no candidate selected (certified quiescence)")
            }
            Err(reason) => {
                let msg = format!("{reason}");
                if json {
                    let res = CliResultJson::failure(
                        cmd_name,
                        Some(FINITE_DECISION_PROFILE.to_string()),
                        Some(run.program.0.to_hex()),
                        Some(context_hex),
                        "unknown",
                        vec![msg],
                    )
                    .with_inputs(input_snapshot, inputs_json);
                    crate::json::emit_result_json(&res);
                } else {
                    eprintln!("brix {cmd_name}: {msg}");
                }
                return EXIT_REJECTED_OR_UNKNOWN;
            }
        }
    };

    // Structured derivation explanation (ADR-0030): purely additive, never
    // changes the status line, exit code, or any field above. A fault or
    // "not found" here is unreachable at this point (both already returned
    // above via `explain_why`/`explain_why_not`), so a failure is swallowed
    // rather than surfaced as a second, redundant error path.
    let explanation = runtime
        .explain_candidate(candidate)
        .ok()
        .and_then(|outcome| match outcome {
            brix_lower::finite_decision::ExplainOutcome::Explained(expl) => Some(*expl),
            brix_lower::finite_decision::ExplainOutcome::CandidateNotFound => None,
        });

    if json {
        let winning_name = run.decision.as_ref().map(|d| d.candidate.as_str());
        let facts_json = run.facts.iter().map(fact_to_json).collect();
        let candidates_json = run
            .dispositions
            .iter()
            .map(|d| candidate_disposition_to_json(d, winning_name))
            .collect();
        let decision_json = run.decision.as_ref().map(decision_to_json);
        let explanation_json = explanation
            .as_ref()
            .map(crate::commands::explain_render::explanation_to_json);

        let res = CliResultJson {
            schema: BRIX_CLI_SCHEMA.to_string(),
            command: cmd_name.to_string(),
            ok: true,
            profile: Some(FINITE_DECISION_PROFILE.to_string()),
            program: Some(run.program.0.to_hex()),
            context: Some(context_hex),
            input_snapshot,
            status: "explained".to_string(),
            inputs: inputs_json,
            facts: facts_json,
            candidates: candidates_json,
            decision: decision_json,
            artifacts: Vec::new(),
            diagnostics: vec![explanation_text],
            bindings: None,
            explanation: explanation_json,
            locations: None,
            shows: None,
            commits: None,
            entity_decisions: None,
        };
        crate::json::emit_result_json(&res);
    } else {
        println!("{explanation_text}");
        let human =
            format_finite_decision_human(&run, Some(&context_hex), input_snapshot.as_deref());
        print!("{human}");
        if let Some(expl) = &explanation {
            print!(
                "{}",
                crate::commands::explain_render::render_explanation_human(expl)
            );
        }
    }

    EXIT_SUCCESS
}

/// Explain a `decide`-scoped candidate (ADR-0043): resolves `--entity` to
/// one settled instance of `decide`'s own block, then reports that
/// instance's own admission/selection against its own admitted set and
/// winner — never a sibling instance's. Uses the dispositions `run` already
/// computed (the same fresh-from-inputs deliberation the commit-pool path
/// re-derives via `explain_why`/`explain_why_not`, just read directly
/// rather than re-run a second time), so it cannot disagree with `brix
/// run`'s own output for the same inputs.
///
/// This is deliberately a simpler report than the commit-pool path's: it
/// does not build the structured, bounded derivation trace
/// (`FiniteDecisionRuntime::explain_candidate`) that path additionally
/// offers, which is scoped to top-level candidates today.
#[allow(clippy::too_many_arguments)]
fn explain_entity(
    cmd_name: &str,
    decide: &brix_lower::finite_decision::FiniteDecisionDecide,
    candidate: &str,
    entity: Option<usize>,
    run: &brix_lower::finite_decision::FiniteDecisionRun,
    context_hex: &str,
    input_snapshot: Option<String>,
    inputs_json: Option<Vec<crate::json::InputJson>>,
    json: bool,
    is_whynot: bool,
) -> u8 {
    let fail = |status: &str, msg: String| -> u8 {
        if json {
            let res = CliResultJson::failure(
                cmd_name,
                Some(FINITE_DECISION_PROFILE.to_string()),
                Some(run.program.0.to_hex()),
                Some(context_hex.to_string()),
                status,
                vec![msg],
            )
            .with_inputs(input_snapshot.clone(), inputs_json.clone());
            crate::json::emit_result_json(&res);
        } else {
            eprintln!("brix {cmd_name}: {msg}");
        }
        if status == "usage-error" {
            EXIT_USAGE_OR_IO
        } else {
            EXIT_REJECTED_OR_UNKNOWN
        }
    };

    let Some(index) = entity else {
        return fail(
            "usage-error",
            format!(
                "'{candidate}' is declared inside decide block '{}'; pass '--entity INDEX' to select which instance to explain",
                decide.name
            ),
        );
    };

    let Some(decide_run) = run.decide_run(&decide.name) else {
        return fail(
            "unknown",
            format!("decide block '{}' has no run to explain", decide.name),
        );
    };
    if let brix_lower::finite_decision::FiniteDecisionDecideStop::Unknown(reason) = &decide_run.stop
    {
        let (code, detail) = unknown_reason_to_code_and_detail(reason);
        return fail("unknown", format!("{code}: {detail}"));
    }
    let Some(inst) = decide_run.instances.get(index) else {
        return fail(
            "usage-error",
            format!(
                "decide block '{}' has {} instance(s); index {index} is out of range",
                decide.name,
                decide_run.instances.len()
            ),
        );
    };
    let Some(disposition) = inst.dispositions.iter().find(|d| d.name == candidate) else {
        return fail(
            "candidate-not-found",
            format!(
                "candidate '{candidate}' not found in decide block '{}' instance {index}",
                decide.name
            ),
        );
    };

    let winning_name = inst.decision.as_ref().map(|d| d.candidate.as_str());
    let cj = candidate_disposition_to_json(disposition, winning_name);
    let explanation_text = if !is_whynot {
        match &disposition.status {
            soc_regimes::finite_frontier::CandidateStatus::Selected => format!(
                "{candidate} [entity {index}]: selected — admitted with minimal calendar key"
            ),
            soc_regimes::finite_frontier::CandidateStatus::AdmittedNotSelected => format!(
                "{candidate} [entity {index}]: admitted-not-selected — overshadowed by selected candidate '{}'",
                winning_name.unwrap_or("(none)")
            ),
            soc_regimes::finite_frontier::CandidateStatus::RejectedGuardFalse => format!(
                "{candidate} [entity {index}]: not-admitted — rejected (guard evaluated to false)"
            ),
            soc_regimes::finite_frontier::CandidateStatus::Rejected(r) => {
                format!("{candidate} [entity {index}]: not-admitted — rejected ({r})")
            }
        }
    } else {
        match &disposition.status {
            soc_regimes::finite_frontier::CandidateStatus::Selected => format!(
                "{candidate} [entity {index}]: actually-selected — candidate was admitted and selected"
            ),
            soc_regimes::finite_frontier::CandidateStatus::AdmittedNotSelected => format!(
                "{candidate} [entity {index}]: overshadowed — admitted but lost selection to higher-priority candidate '{}'",
                winning_name.unwrap_or("(none)")
            ),
            soc_regimes::finite_frontier::CandidateStatus::RejectedGuardFalse => format!(
                "{candidate} [entity {index}]: rejected — guard evaluated to false"
            ),
            soc_regimes::finite_frontier::CandidateStatus::Rejected(r) => {
                format!("{candidate} [entity {index}]: rejected — {r}")
            }
        }
    };

    if json {
        let res = CliResultJson {
            schema: BRIX_CLI_SCHEMA.to_string(),
            command: cmd_name.to_string(),
            ok: true,
            profile: Some(FINITE_DECISION_PROFILE.to_string()),
            program: Some(run.program.0.to_hex()),
            context: Some(context_hex.to_string()),
            input_snapshot,
            status: "explained".to_string(),
            inputs: inputs_json,
            facts: run.facts.iter().map(fact_to_json).collect(),
            candidates: vec![cj],
            decision: inst.decision.as_ref().map(decision_to_json),
            artifacts: Vec::new(),
            diagnostics: vec![explanation_text],
            explanation: None,
            locations: None,
            shows: None,
            commits: None,
            entity_decisions: Some(vec![crate::commands::decide_run_to_json(decide_run)]),
            bindings: None,
        };
        crate::json::emit_result_json(&res);
    } else {
        println!("{explanation_text}");
        println!("binder: {}", crate::commands::fmt_value_human(&inst.binder));
        if let Some(sel) = &inst.decision {
            println!(
                "decision: {} = {} @Derived",
                sel.candidate,
                crate::commands::fmt_value_human(&sel.value)
            );
        } else {
            println!("decision: none (quiescent)");
        }
        println!("program: {}", run.program.0.to_hex());
        println!("context: {context_hex}");
    }

    EXIT_SUCCESS
}
