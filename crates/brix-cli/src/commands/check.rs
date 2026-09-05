//! `brix check` — parse, resolve imports, lower, and preflight check a Brix module.

use std::path::{Path, PathBuf};

use brix_lower::check_module;
use brix_lower::finite_decision::{
    lower_finite_decision_plan, FiniteDecisionRuntime, FiniteDecisionStop, FINITE_DECISION_PROFILE,
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

/// Execute `brix check <file.brix>`.
pub fn execute_check(file: &Path, json: bool, package_paths: &[PathBuf]) -> u8 {
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
        .any(|i| matches!(i, Item::Commit(_) | Item::Propose(_)));

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

        // Preflight: run the plan so check cannot exit success when run would return Unknown.
        let runtime = FiniteDecisionRuntime::build(&plan);
        let context_hex = runtime.context.digest().to_hex();
        let run = runtime.run();

        if run.is_unknown() {
            let (code, detail) = match &run.stop {
                FiniteDecisionStop::Unknown(reason) => unknown_reason_to_code_and_detail(reason),
                _ => unreachable!(),
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
                );
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
                status: "accepted".to_string(),
                facts: facts_json,
                candidates: candidates_json,
                decision: decision_json,
                artifacts: Vec::new(),
                diagnostics: Vec::new(),
            };
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
        } else {
            let human = format_finite_decision_human(&run, Some(&context_hex));
            print!("{human}");
        }

        return EXIT_SUCCESS;
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
            status: if had_error {
                "rejected".to_string()
            } else {
                "accepted".to_string()
            },
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
