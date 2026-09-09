//! `brix run` — finite-decision deliberation execution (ADR-0030).

use std::path::{Path, PathBuf};

use brix_lower::finite_decision::{
    finite_decision_program_id, lower_finite_decision_plan, FiniteDecisionRuntime,
    FiniteDecisionStop, FINITE_DECISION_PROFILE,
};
use brix_syntax::parse_bounded;

use crate::cli::{EXIT_REJECTED_OR_UNKNOWN, EXIT_SUCCESS, EXIT_USAGE_OR_IO};
use crate::commands::{
    candidate_disposition_to_json, decision_to_json, fact_to_json, format_finite_decision_human,
    unknown_reason_to_code_and_detail,
};
use crate::json::{CliResultJson, BRIX_CLI_SCHEMA};
use crate::packages::{make_package_loader, read_source_bounded};

/// Execute `brix run <file.brix> [--input <path>...]`.
pub fn execute_run(
    file: &Path,
    json: bool,
    package_paths: &[PathBuf],
    input_paths: &[PathBuf],
) -> u8 {
    let source = match read_source_bounded(file) {
        Ok(s) => s,
        Err(err) => {
            if json {
                let res = CliResultJson::failure("run", None, None, None, "io-error", vec![err]);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix run: {err}");
            }
            return EXIT_USAGE_OR_IO;
        }
    };

    let module = match parse_bounded(&source, brix_syntax::ParseLimits::strict()) {
        Ok(m) => m,
        Err(err) => {
            let msg = format!("parse error: {err}");
            if json {
                let res = CliResultJson::failure("run", None, None, None, "rejected", vec![msg]);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix run: rejected: {msg}");
            }
            return EXIT_REJECTED_OR_UNKNOWN;
        }
    };

    let loader = make_package_loader(package_paths);
    let mut resolved_module = match brix_lower::imports::resolve_imports(&module, &loader) {
        Ok(m) => m,
        Err(err) => {
            let msg = format!("import error: {err:?}");
            if json {
                let res = CliResultJson::failure("run", None, None, None, "rejected", vec![msg]);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix run: rejected: {msg}");
            }
            return EXIT_REJECTED_OR_UNKNOWN;
        }
    };

    crate::commands::prepare_finite_decision_module(&mut resolved_module);

    let plan = match lower_finite_decision_plan(&resolved_module, FINITE_DECISION_PROFILE) {
        Ok(p) => p,
        Err(err) => {
            let msg = format!("lowering error: {err}");
            if json {
                let res = CliResultJson::failure("run", None, None, None, "rejected", vec![msg]);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix run: rejected: {msg}");
            }
            return EXIT_REJECTED_OR_UNKNOWN;
        }
    };

    let snapshot = match crate::commands::load_cli_input_snapshot(input_paths) {
        Ok(s) => s,
        Err(err) => {
            if json {
                let res = CliResultJson::failure(
                    "run",
                    Some(FINITE_DECISION_PROFILE.to_string()),
                    Some(finite_decision_program_id(&plan).0.to_hex()),
                    None,
                    err.status(),
                    vec![err.diagnostic()],
                );
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("{}", err.render_human("run"));
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
                    "run",
                    Some(FINITE_DECISION_PROFILE.to_string()),
                    Some(finite_decision_program_id(&plan).0.to_hex()),
                    None,
                    cli_err.status(),
                    vec![cli_err.diagnostic()],
                )
                .with_inputs(snapshot_hex, None);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("{}", cli_err.render_human("run"));
            }
            return cli_err.exit_code();
        }
    };
    let context_hex = runtime.context.digest().to_hex();
    let run = runtime.run();

    let is_ok = !run.is_unknown();
    let (status_str, diagnostics) = match &run.stop {
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
            command: "run".to_string(),
            ok: is_ok,
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
        };
        println!("{}", serde_json::to_string_pretty(&res).unwrap());
    } else {
        let human =
            format_finite_decision_human(&run, Some(&context_hex), input_snapshot.as_deref());
        print!("{human}");
    }

    if is_ok {
        EXIT_SUCCESS
    } else {
        EXIT_REJECTED_OR_UNKNOWN
    }
}
