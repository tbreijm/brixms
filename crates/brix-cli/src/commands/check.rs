//! `brix check` — parse, resolve imports, lower, and preflight check a Brix module.

use std::path::{Path, PathBuf};

use brix_lower::check_module;
use brix_lower::finite_decision::{
    finite_decision_program_id, lower_finite_decision_plan, FiniteDecisionRuntime,
    FiniteDecisionStop, FINITE_DECISION_PROFILE,
};
use brix_syntax::ast::Item;
use brix_syntax::parse_bounded;

use crate::cli::{EXIT_REJECTED_OR_UNKNOWN, EXIT_SUCCESS, EXIT_USAGE_OR_IO};
use crate::commands::{
    candidate_disposition_to_json, decision_to_json, fact_to_json, format_finite_decision_human,
    unknown_reason_to_code_and_detail,
};
use crate::json::{CliResultJson, BRIX_CLI_SCHEMA};
use crate::packages::{make_package_loader, read_source_bounded};

/// Execute `brix check <file.brix> [--input <path>...]`.
pub fn execute_check(
    file: &Path,
    json: bool,
    package_paths: &[PathBuf],
    input_paths: &[PathBuf],
) -> u8 {
    let source = match read_source_bounded(file) {
        Ok(s) => s,
        Err(err) => {
            if json {
                let res = CliResultJson::failure("check", None, None, None, "io-error", vec![err]);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix check: {err}");
            }
            return EXIT_USAGE_OR_IO;
        }
    };

    let module = match parse_bounded(&source, brix_syntax::ParseLimits::strict()) {
        Ok(m) => m,
        Err(err) => {
            let msg = format!("parse error: {err}");
            if json {
                let res = CliResultJson::failure("check", None, None, None, "rejected", vec![msg]);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix check: rejected: {msg}");
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
                let res = CliResultJson::failure("check", None, None, None, "rejected", vec![msg]);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix check: rejected: {msg}");
            }
            return EXIT_REJECTED_OR_UNKNOWN;
        }
    };

    // Determine if the module is in the finite-decision profile fragment.
    let has_finite_decision_items = resolved_module
        .items
        .iter()
        .any(|i| matches!(i, Item::Commit(_) | Item::Propose(_) | Item::Input(_)));

    if has_finite_decision_items {
        let mut resolved_module = resolved_module;
        crate::commands::prepare_finite_decision_module(&mut resolved_module);
        // Lower as finite-decision
        let plan = match lower_finite_decision_plan(&resolved_module, FINITE_DECISION_PROFILE) {
            Ok(p) => p,
            Err(err) => {
                let msg = format!("lowering error: {err}");
                if json {
                    let res = CliResultJson::failure(
                        "check",
                        Some(FINITE_DECISION_PROFILE.to_string()),
                        None,
                        None,
                        "rejected",
                        vec![msg],
                    );
                    println!("{}", serde_json::to_string_pretty(&res).unwrap());
                } else {
                    eprintln!("brix check: rejected: {msg}");
                }
                return EXIT_REJECTED_OR_UNKNOWN;
            }
        };

        // If source declares inputs and NO --input was supplied:
        // Validate syntax and lowering/declarations only, do NOT build runtime or run preflight.
        if !plan.inputs.is_empty() && input_paths.is_empty() {
            let prog_id = finite_decision_program_id(&plan).0.to_hex();
            if json {
                let res = CliResultJson {
                    schema: BRIX_CLI_SCHEMA.to_string(),
                    command: "check".to_string(),
                    ok: true,
                    profile: Some(FINITE_DECISION_PROFILE.to_string()),
                    program: Some(prog_id),
                    context: None,
                    input_snapshot: None,
                    status: "checked-input-contract".to_string(),
                    inputs: None,
                    facts: Vec::new(),
                    candidates: Vec::new(),
                    decision: None,
                    artifacts: Vec::new(),
                    diagnostics: Vec::new(),
                };
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                println!("status: checked-input-contract");
                println!("program: {prog_id}");
            }
            return EXIT_SUCCESS;
        }

        // Load input snapshot from provided paths (or empty snapshot if none).
        let snapshot = match crate::commands::load_cli_input_snapshot(input_paths) {
            Ok(s) => s,
            Err(err) => {
                if json {
                    let res = CliResultJson::failure(
                        "check",
                        Some(FINITE_DECISION_PROFILE.to_string()),
                        Some(finite_decision_program_id(&plan).0.to_hex()),
                        None,
                        err.status(),
                        vec![err.diagnostic()],
                    );
                    println!("{}", serde_json::to_string_pretty(&res).unwrap());
                } else {
                    eprintln!("{}", err.render_human("check"));
                }
                return err.exit_code();
            }
        };

        // Preflight: build runtime with inputs and run the plan so check cannot exit success when run would return Unknown.
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
                        "check",
                        Some(FINITE_DECISION_PROFILE.to_string()),
                        Some(finite_decision_program_id(&plan).0.to_hex()),
                        None,
                        cli_err.status(),
                        vec![cli_err.diagnostic()],
                    )
                    .with_inputs(snapshot_hex, None);
                    println!("{}", serde_json::to_string_pretty(&res).unwrap());
                } else {
                    eprintln!("{}", cli_err.render_human("check"));
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
                FiniteDecisionStop::Unknown(reason) => unknown_reason_to_code_and_detail(reason),
                _ => (
                    "unknown-stop",
                    "unexpected deliberation stop condition in failure path".to_string(),
                ),
            };
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
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
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
            };
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
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
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
        } else {
            eprintln!("brix check: {msg}");
        }
        return EXIT_USAGE_OR_IO;
    }

    // Classic L1/L2 module check path (check_module)
    let results = check_module(&resolved_module);
    let mut had_error = false;
    let mut diagnostics = Vec::new();
    let mut human_lines = Vec::new();

    if results.is_empty() {
        human_lines.push("(no `let` bindings to check)".to_string());
    } else {
        for r in &results {
            match r {
                Ok(cr) => {
                    human_lines.push(format!("  {} : — @{:?}", cr.name, cr.outcome));
                }
                Err((name, err)) => {
                    had_error = true;
                    let msg = format!("{name}: not checked: {err:?}");
                    diagnostics.push(msg.clone());
                    human_lines.push(format!("  {msg}"));
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
        };
        println!("{}", serde_json::to_string_pretty(&res).unwrap());
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
