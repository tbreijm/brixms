//! `brix audit` — run, audit journal, and atomically write an audit input bundle (ADR-0026, ADR-0030).

use std::io::Write;
use std::path::{Path, PathBuf};

use brix_lower::audit_bundle::produce_finite_decision_audit_input_bundle_v1;
use brix_lower::finite_decision::{
    finite_decision_program_id, lower_finite_decision_plan, FiniteDecisionRuntime,
    FiniteDecisionStop, FINITE_DECISION_PROFILE,
};
use brix_syntax::parse_bounded;
use soc_core::audit::AuditResult;
use soc_core::audit_bundle::AuditDecodeLimits;

use crate::cli::{EXIT_REJECTED_OR_UNKNOWN, EXIT_SUCCESS, EXIT_USAGE_OR_IO};
use crate::commands::{
    candidate_disposition_to_json, decision_to_json, fact_to_json, format_finite_decision_human,
    unknown_reason_to_code_and_detail,
};
use crate::json::{ArtifactJson, CliResultJson, BRIX_CLI_SCHEMA};
use crate::packages::{make_package_loader, read_source_bounded};

/// Execute `brix audit <file.brix> --bundle <out> [--force] [--input <path>...]`.
pub fn execute_audit(
    file: &Path,
    bundle_out: &Path,
    force: bool,
    json: bool,
    package_paths: &[PathBuf],
    input_paths: &[PathBuf],
) -> u8 {
    // Refusal: if bundle destination already exists and force was not specified, refuse immediately.
    if bundle_out.exists() && !force {
        let msg = format!(
            "destination file '{}' already exists (use --force to overwrite)",
            bundle_out.display()
        );
        if json {
            let res = CliResultJson::failure("audit", None, None, None, "io-error", vec![msg]);
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
        } else {
            eprintln!("brix audit: refused: {msg}");
        }
        return EXIT_USAGE_OR_IO;
    }

    let source = match read_source_bounded(file) {
        Ok(s) => s,
        Err(err) => {
            if json {
                let res = CliResultJson::failure("audit", None, None, None, "io-error", vec![err]);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix audit: {err}");
            }
            return EXIT_USAGE_OR_IO;
        }
    };

    let module = match parse_bounded(&source, brix_syntax::ParseLimits::strict()) {
        Ok(m) => m,
        Err(err) => {
            let msg = format!("parse error: {err}");
            if json {
                let res = CliResultJson::failure("audit", None, None, None, "rejected", vec![msg]);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix audit: rejected: {msg}");
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
                let res = CliResultJson::failure("audit", None, None, None, "rejected", vec![msg]);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix audit: rejected: {msg}");
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
                let res = CliResultJson::failure("audit", None, None, None, "rejected", vec![msg]);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix audit: rejected: {msg}");
            }
            return EXIT_REJECTED_OR_UNKNOWN;
        }
    };

    let snapshot = match crate::commands::load_cli_input_snapshot(input_paths) {
        Ok(s) => s,
        Err(err) => {
            if json {
                let res = CliResultJson::failure(
                    "audit",
                    Some(FINITE_DECISION_PROFILE.to_string()),
                    Some(finite_decision_program_id(&plan).0.to_hex()),
                    None,
                    err.status(),
                    vec![err.diagnostic()],
                );
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("{}", err.render_human("audit"));
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
                    "audit",
                    Some(FINITE_DECISION_PROFILE.to_string()),
                    Some(finite_decision_program_id(&plan).0.to_hex()),
                    None,
                    cli_err.status(),
                    vec![cli_err.diagnostic()],
                )
                .with_inputs(snapshot_hex, None);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("{}", cli_err.render_human("audit"));
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

    // The run itself must not be Unknown.
    if run.is_unknown() {
        let (code, detail) = match &run.stop {
            FiniteDecisionStop::Unknown(reason) => unknown_reason_to_code_and_detail(reason),
            _ => (
                "unknown-stop",
                "unexpected deliberation stop condition in failure path".to_string(),
            ),
        };
        if json {
            let res = CliResultJson::failure(
                "audit",
                Some(FINITE_DECISION_PROFILE.to_string()),
                Some(run.program.0.to_hex()),
                Some(context_hex),
                "unknown",
                vec![format!("{code}: {detail}")],
            )
            .with_inputs(input_snapshot, inputs_json);
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
        } else {
            let human =
                format_finite_decision_human(&run, Some(&context_hex), input_snapshot.as_deref());
            print!("{human}");
        }
        return EXIT_REJECTED_OR_UNKNOWN;
    }

    // Audit each step in the journal independently.
    let audit_results = runtime.audit(&run.journal);
    let mut audit_unknown_count = 0usize;
    let mut audit_lines = Vec::new();
    for (i, res) in audit_results.iter().enumerate() {
        match res {
            AuditResult::Audited(a) => {
                audit_lines.push(format!("audit[{i}]: audited {}", a.audited_id.to_hex()));
            }
            AuditResult::Unknown(reason) => {
                audit_unknown_count += 1;
                audit_lines.push(format!("audit[{i}]: unknown ({reason})"));
            }
        }
    }

    if audit_unknown_count > 0 {
        let msg = format!(
            "{audit_unknown_count} of {} steps unknown",
            audit_results.len()
        );
        if json {
            let res = CliResultJson::failure(
                "audit",
                Some(FINITE_DECISION_PROFILE.to_string()),
                Some(run.program.0.to_hex()),
                Some(context_hex),
                "unknown",
                audit_lines,
            )
            .with_inputs(input_snapshot, inputs_json);
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
        } else {
            let mut human =
                format_finite_decision_human(&run, Some(&context_hex), input_snapshot.as_deref());
            for line in &audit_lines {
                human.push_str(&format!("{line}\n"));
            }
            human.push_str(&format!("status: unknown ({msg})\n"));
            print!("{human}");
        }
        return EXIT_REJECTED_OR_UNKNOWN;
    }

    // Produce canonical audit input bundle.
    let bundle = match produce_finite_decision_audit_input_bundle_v1(&runtime, &run) {
        Ok(b) => b,
        Err(err) => {
            let msg = format!("bundle production error: {err}");
            if json {
                let res = CliResultJson::failure(
                    "audit",
                    Some(FINITE_DECISION_PROFILE.to_string()),
                    Some(run.program.0.to_hex()),
                    Some(context_hex),
                    "unknown",
                    vec![msg],
                )
                .with_inputs(input_snapshot, inputs_json);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix audit: {msg}");
            }
            return EXIT_REJECTED_OR_UNKNOWN;
        }
    };

    let bundle_bytes = match bundle.encode(&AuditDecodeLimits::strict()) {
        Ok(bytes) => bytes,
        Err(err) => {
            let msg = format!("bundle encoding error: {err:?}");
            if json {
                let res = CliResultJson::failure(
                    "audit",
                    Some(FINITE_DECISION_PROFILE.to_string()),
                    Some(run.program.0.to_hex()),
                    Some(context_hex),
                    "unknown",
                    vec![msg],
                )
                .with_inputs(input_snapshot, inputs_json);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix audit: {msg}");
            }
            return EXIT_REJECTED_OR_UNKNOWN;
        }
    };

    // Race-safe creation, atomic rename, cleanup on every failure in the same directory.
    let parent = bundle_out.parent().unwrap_or_else(|| Path::new("."));
    if !parent.as_os_str().is_empty() && !parent.exists() {
        let msg = format!("parent directory '{}' does not exist", parent.display());
        if json {
            let res = CliResultJson::failure("audit", None, None, None, "io-error", vec![msg]);
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
        } else {
            eprintln!("brix audit: IO error: {msg}");
        }
        return EXIT_USAGE_OR_IO;
    }

    struct TempFileGuard<'a> {
        path: &'a Path,
        disarmed: bool,
    }

    impl<'a> Drop for TempFileGuard<'a> {
        fn drop(&mut self) {
            if !self.disarmed && self.path.exists() {
                let _ = std::fs::remove_file(self.path);
            }
        }
    }

    let mut temp_pair = None;
    for attempt in 0..1000 {
        let temp_name = format!(
            ".tmp_bundle_{}_{}_{}.tmp",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            attempt
        );
        let temp_path = parent.join(temp_name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => {
                temp_pair = Some((file, temp_path));
                break;
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                let msg = format!("cannot create temporary bundle file: {err}");
                if json {
                    let res =
                        CliResultJson::failure("audit", None, None, None, "io-error", vec![msg]);
                    println!("{}", serde_json::to_string_pretty(&res).unwrap());
                } else {
                    eprintln!("brix audit: IO error: {msg}");
                }
                return EXIT_USAGE_OR_IO;
            }
        }
    }

    let (mut f, temp_path) = match temp_pair {
        Some(p) => p,
        None => {
            let msg = "failed to create unique temporary file after 1000 attempts".to_string();
            if json {
                let res = CliResultJson::failure("audit", None, None, None, "io-error", vec![msg]);
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix audit: IO error: {msg}");
            }
            return EXIT_USAGE_OR_IO;
        }
    };

    let mut guard = TempFileGuard {
        path: &temp_path,
        disarmed: false,
    };

    let write_result = (|| -> Result<(), std::io::Error> {
        f.write_all(&bundle_bytes)?;
        f.flush()?;
        f.sync_all()?;
        drop(f);

        if bundle_out.exists() && !force {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "destination file '{}' already exists (use --force to overwrite)",
                    bundle_out.display()
                ),
            ));
        }

        std::fs::rename(&temp_path, bundle_out)?;
        guard.disarmed = true;
        Ok(())
    })();

    if let Err(err) = write_result {
        let msg = format!(
            "cannot write bundle file to '{}': {err}",
            bundle_out.display()
        );
        if json {
            let res = CliResultJson::failure("audit", None, None, None, "io-error", vec![msg]);
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
        } else {
            eprintln!("brix audit: IO error: {msg}");
        }
        return EXIT_USAGE_OR_IO;
    }

    let bundle_id_hex = bundle.id().digest().to_hex();
    let final_chain_hex = bundle.final_chain_digest.to_hex();
    let receipts_count = bundle.entries.len();

    let (_context, registry, semantics) = runtime.audit_environment();
    let receipt_ids_vec: Option<Vec<String>> = soc_core::audit_bundle::check_audit_input_bundle_v1(
        &bundle,
        &registry,
        &semantics,
        &AuditDecodeLimits::strict(),
    )
    .ok()
    .map(|ids| ids.iter().map(|r| r.digest().to_hex()).collect());

    let artifact = ArtifactJson::bundle(
        Some(bundle_out.display().to_string()),
        bundle_id_hex.clone(),
        final_chain_hex.clone(),
        receipt_ids_vec,
        receipts_count.to_string(),
    );

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
            command: "audit".to_string(),
            ok: true,
            profile: Some(FINITE_DECISION_PROFILE.to_string()),
            program: Some(run.program.0.to_hex()),
            context: Some(context_hex),
            input_snapshot,
            status: "audited".to_string(),
            inputs: inputs_json,
            facts: facts_json,
            candidates: candidates_json,
            decision: decision_json,
            artifacts: vec![artifact],
            diagnostics: Vec::new(),
        };
        println!("{}", serde_json::to_string_pretty(&res).unwrap());
    } else {
        let mut human =
            format_finite_decision_human(&run, Some(&context_hex), input_snapshot.as_deref());
        for line in audit_lines {
            human.push_str(&format!("{line}\n"));
        }
        human.push_str("status: audited\n");
        human.push_str("audit: audited\n");
        human.push_str(&format!("bundle: {}\n", bundle_out.display()));
        human.push_str(&format!("bundle_id: {bundle_id_hex}\n"));
        human.push_str(&format!("final_chain: {final_chain_hex}\n"));
        print!("{human}");
    }

    EXIT_SUCCESS
}
