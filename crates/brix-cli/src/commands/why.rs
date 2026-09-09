//! `brix why` and `brix whynot` — deliberation explanation re-derivation (ADR-0030).

use std::path::{Path, PathBuf};

use brix_lower::finite_decision::{
    finite_decision_program_id, lower_finite_decision_plan, FiniteDecisionRuntime,
    FINITE_DECISION_PROFILE,
};
use brix_syntax::parse_bounded;
use soc_regimes::finite_frontier::{WhyExplanation, WhyNotExplanation};

use crate::cli::{EXIT_REJECTED_OR_UNKNOWN, EXIT_SUCCESS, EXIT_USAGE_OR_IO};
use crate::commands::{
    candidate_disposition_to_json, decision_to_json, fact_to_json, format_finite_decision_human,
    unknown_reason_to_code_and_detail,
};
use crate::json::{CliResultJson, BRIX_CLI_SCHEMA};
use crate::packages::{make_package_loader, read_source_bounded};

/// Execute `brix why` or `brix whynot`.
pub fn execute_why_or_whynot(
    file: &Path,
    candidate: &str,
    json: bool,
    package_paths: &[PathBuf],
    input_paths: &[PathBuf],
    is_whynot: bool,
) -> u8 {
    let cmd_name = if is_whynot { "whynot" } else { "why" };

    let source = match read_source_bounded(file) {
        Ok(s) => s,
        Err(err) => {
            if json {
                let res = CliResultJson::failure(cmd_name, None, None, None, "io-error", vec![err]);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix {cmd_name}: {err}");
            }
            return EXIT_USAGE_OR_IO;
        }
    };

    let module = match parse_bounded(&source, brix_syntax::ParseLimits::strict()) {
        Ok(m) => m,
        Err(err) => {
            let msg = format!("parse error: {err}");
            if json {
                let res = CliResultJson::failure(cmd_name, None, None, None, "rejected", vec![msg]);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix {cmd_name}: rejected: {msg}");
            }
            return EXIT_REJECTED_OR_UNKNOWN;
        }
    };

    let loader = make_package_loader(package_paths);
    let resolved_module = match brix_lower::imports::resolve_imports(&module, &loader) {
        Ok(m) => m,
        Err(err) => {
            let msg = format!("import error: {err:?}");
            if json {
                let res = CliResultJson::failure(cmd_name, None, None, None, "rejected", vec![msg]);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix {cmd_name}: rejected: {msg}");
            }
            return EXIT_REJECTED_OR_UNKNOWN;
        }
    };

    let mut resolved_module = resolved_module;
    crate::commands::prepare_finite_decision_module(&mut resolved_module);

    let plan = match lower_finite_decision_plan(&resolved_module, FINITE_DECISION_PROFILE) {
        Ok(p) => p,
        Err(err) => {
            let msg = format!("lowering error: {err}");
            if json {
                let res = CliResultJson::failure(cmd_name, None, None, None, "rejected", vec![msg]);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix {cmd_name}: rejected: {msg}");
            }
            return EXIT_REJECTED_OR_UNKNOWN;
        }
    };

    let snapshot = match crate::commands::load_cli_input_snapshot(input_paths) {
        Ok(s) => s,
        Err(err) => {
            if json {
                let res = CliResultJson::failure(
                    cmd_name,
                    Some(FINITE_DECISION_PROFILE.to_string()),
                    Some(finite_decision_program_id(&plan).0.to_hex()),
                    None,
                    err.status(),
                    vec![err.diagnostic()],
                );
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("{}", err.render_human(cmd_name));
            }
            return err.exit_code();
        }
    };

    let runtime = match FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot) {
        Ok(r) => r,
        Err(err) => {
            let cli_err = crate::commands::CliInputError::from(err);
            let snapshot_hex = if !snapshot.is_empty() {
                Some(snapshot.id().0.to_hex())
            } else {
                None
            };
            if json {
                let res = CliResultJson::failure(
                    cmd_name,
                    Some(FINITE_DECISION_PROFILE.to_string()),
                    Some(finite_decision_program_id(&plan).0.to_hex()),
                    None,
                    cli_err.status(),
                    vec![cli_err.diagnostic()],
                )
                .with_inputs(snapshot_hex, None);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("{}", cli_err.render_human(cmd_name));
            }
            return cli_err.exit_code();
        }
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

    if run.is_unknown() {
        let (code, detail) = match &run.stop {
            brix_lower::finite_decision::FiniteDecisionStop::Unknown(reason) => {
                unknown_reason_to_code_and_detail(reason)
            }
            _ => (
                "unknown-stop",
                "unexpected deliberation stop condition in failure path".to_string(),
            ),
        };
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
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
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
                    println!("{}", serde_json::to_string_pretty(&res).unwrap());
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
                    println!("{}", serde_json::to_string_pretty(&res).unwrap());
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
                    println!("{}", serde_json::to_string_pretty(&res).unwrap());
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
                    println!("{}", serde_json::to_string_pretty(&res).unwrap());
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
                    println!("{}", serde_json::to_string_pretty(&res).unwrap());
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
                    println!("{}", serde_json::to_string_pretty(&res).unwrap());
                } else {
                    eprintln!("brix {cmd_name}: {msg}");
                }
                return EXIT_REJECTED_OR_UNKNOWN;
            }
        }
    };

    if json {
        let winning_name = run.decision.as_ref().map(|d| d.candidate.as_str());
        let facts_json = run.facts.iter().map(fact_to_json).collect();
        let candidates_json = run
            .dispositions
            .iter()
            .map(|d| candidate_disposition_to_json(d, winning_name))
            .collect();
        let decision_json = run.decision.as_ref().map(decision_to_json);

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
        };
        println!("{}", serde_json::to_string_pretty(&res).unwrap());
    } else {
        println!("{explanation_text}");
        let human =
            format_finite_decision_human(&run, Some(&context_hex), input_snapshot.as_deref());
        print!("{human}");
    }

    EXIT_SUCCESS
}
