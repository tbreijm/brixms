//! `brix verify` — verify an audit input bundle against source and external expected program pin (ADR-0026, ADR-0030).

use std::io::Read;
use std::path::{Path, PathBuf};

use brix_canon::Digest;
use brix_lower::audit_bundle::{
    check_finite_decision_audit_input_bundle_from_module_v1,
    check_l3_audit_input_bundle_from_module_v1,
};
use brix_lower::finite_decision::FiniteDecisionProgramId;
use brix_lower::l3_canon::ProgramIdV1;
use brix_lower::PlanLimitsV1;
use brix_syntax::parse_bounded;
use soc_core::audit_bundle::{decode_audit_input_bundle_v1, AuditDecodeLimits};

use crate::cli::{VerifyProfile, EXIT_REJECTED_OR_UNKNOWN, EXIT_SUCCESS, EXIT_USAGE_OR_IO};
use crate::json::{ArtifactJson, CliResultJson, BRIX_CLI_SCHEMA};
use crate::packages::{make_package_loader, read_source_bounded};

/// Execute `brix verify --expect-program <hex> <file.brix> <bundle> [--profile <finite-decision|l3-v1>]`.
pub fn execute_verify(
    expect_program_hex: &str,
    file: &Path,
    bundle_path: &Path,
    profile: VerifyProfile,
    json: bool,
    package_paths: &[PathBuf],
) -> u8 {
    let profile_str = match profile {
        VerifyProfile::FiniteDecision => brix_lower::FINITE_DECISION_PROFILE,
        VerifyProfile::L3V1 => brix_lower::L3_PROFILE_MARKER_V1,
    };

    // 1. Parse strict external 64-hex program pin without deriving it from inputs.
    let pin_bytes = match hex_to_32_bytes(expect_program_hex) {
        Ok(b) => b,
        Err(err) => {
            let msg = format!("invalid hex pin '{expect_program_hex}': {err}");
            if json {
                let res = CliResultJson::failure(
                    "verify",
                    Some(profile_str.to_string()),
                    None,
                    None,
                    "usage-error",
                    vec![msg],
                );
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix verify: usage error: {msg}");
            }
            return EXIT_USAGE_OR_IO;
        }
    };

    // 2. Check bundle size before whole-file read.
    let decode_limits = AuditDecodeLimits::strict();
    let bundle_meta = match std::fs::metadata(bundle_path) {
        Ok(m) => m,
        Err(err) => {
            let msg = format!("cannot read bundle file '{}': {err}", bundle_path.display());
            if json {
                let res = CliResultJson::failure(
                    "verify",
                    Some(profile_str.to_string()),
                    Some(expect_program_hex.to_string()),
                    None,
                    "io-error",
                    vec![msg],
                );
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix verify: IO error: {msg}");
            }
            return EXIT_USAGE_OR_IO;
        }
    };

    if bundle_meta.len() > decode_limits.max_total_bundle_bytes as u64 {
        let msg = format!(
            "bundle file '{}' exceeds maximum allowed size ({} bytes > {} limit)",
            bundle_path.display(),
            bundle_meta.len(),
            decode_limits.max_total_bundle_bytes
        );
        if json {
            let res = CliResultJson::failure(
                "verify",
                Some(profile_str.to_string()),
                Some(expect_program_hex.to_string()),
                None,
                "io-error",
                vec![msg],
            );
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
        } else {
            eprintln!("brix verify: IO error: {msg}");
        }
        return EXIT_USAGE_OR_IO;
    }

    let mut bundle_file = match std::fs::File::open(bundle_path) {
        Ok(f) => f,
        Err(err) => {
            let msg = format!("cannot open bundle file '{}': {err}", bundle_path.display());
            if json {
                let res = CliResultJson::failure(
                    "verify",
                    Some(profile_str.to_string()),
                    Some(expect_program_hex.to_string()),
                    None,
                    "io-error",
                    vec![msg],
                );
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix verify: IO error: {msg}");
            }
            return EXIT_USAGE_OR_IO;
        }
    };

    let mut bundle_bytes = Vec::new();
    if let Err(err) = bundle_file
        .by_ref()
        .take((decode_limits.max_total_bundle_bytes + 1) as u64)
        .read_to_end(&mut bundle_bytes)
    {
        let msg = format!("cannot read bundle file '{}': {err}", bundle_path.display());
        if json {
            let res = CliResultJson::failure(
                "verify",
                Some(profile_str.to_string()),
                Some(expect_program_hex.to_string()),
                None,
                "io-error",
                vec![msg],
            );
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
        } else {
            eprintln!("brix verify: IO error: {msg}");
        }
        return EXIT_USAGE_OR_IO;
    }

    if bundle_bytes.len() > decode_limits.max_total_bundle_bytes {
        let msg = format!(
            "bundle file '{}' exceeds maximum allowed size ({} bytes > {} limit)",
            bundle_path.display(),
            bundle_bytes.len(),
            decode_limits.max_total_bundle_bytes
        );
        if json {
            let res = CliResultJson::failure(
                "verify",
                Some(profile_str.to_string()),
                Some(expect_program_hex.to_string()),
                None,
                "io-error",
                vec![msg],
            );
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
        } else {
            eprintln!("brix verify: IO error: {msg}");
        }
        return EXIT_USAGE_OR_IO;
    }

    // 3. Decodes and bounds bundle before source work.
    let decoded_bundle = match decode_audit_input_bundle_v1(&bundle_bytes, &decode_limits) {
        Ok(b) => b,
        Err(err) => {
            let msg = format!("bundle decode error: {err:?}");
            if json {
                let res = CliResultJson::failure(
                    "verify",
                    Some(profile_str.to_string()),
                    Some(expect_program_hex.to_string()),
                    None,
                    "unknown",
                    vec![format!("bundle-decode-error: {err:?}")],
                );
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix verify: unknown ({msg})");
            }
            return EXIT_REJECTED_OR_UNKNOWN;
        }
    };

    // 4. Read source applying strict 1 MiB limit before allocation.
    let source = match read_source_bounded(file) {
        Ok(s) => s,
        Err(err) => {
            if json {
                let res = CliResultJson::failure(
                    "verify",
                    Some(profile_str.to_string()),
                    Some(expect_program_hex.to_string()),
                    None,
                    "io-error",
                    vec![err],
                );
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix verify: {err}");
            }
            return EXIT_USAGE_OR_IO;
        }
    };

    // 5. Parse and resolve module.
    let module = match parse_bounded(&source, brix_syntax::ParseLimits::strict()) {
        Ok(m) => m,
        Err(err) => {
            let msg = format!("source parse error: {err}");
            if json {
                let res = CliResultJson::failure(
                    "verify",
                    Some(profile_str.to_string()),
                    Some(expect_program_hex.to_string()),
                    None,
                    "unknown",
                    vec![format!("source-parse-error: {err}")],
                );
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix verify: unknown ({msg})");
            }
            return EXIT_REJECTED_OR_UNKNOWN;
        }
    };

    let loader = make_package_loader(package_paths);
    let resolved_module = match brix_lower::imports::resolve_imports(&module, &loader) {
        Ok(m) => m,
        Err(err) => {
            let msg = format!("source import error: {err:?}");
            if json {
                let res = CliResultJson::failure(
                    "verify",
                    Some(profile_str.to_string()),
                    Some(expect_program_hex.to_string()),
                    None,
                    "unknown",
                    vec![format!("source-import-error: {err:?}")],
                );
                println!("{}", serde_json::to_string_pretty(&res).unwrap());
            } else {
                eprintln!("brix verify: unknown ({msg})");
            }
            return EXIT_REJECTED_OR_UNKNOWN;
        }
    };

    // 6. Call the correct source/module verifier.
    let plan_limits = PlanLimitsV1::generous();

    let (
        context_hex,
        bundle_id_hex,
        final_chain_hex,
        receipts_count,
        receipt_lines,
        receipt_ids_hex,
    ) = match profile {
        VerifyProfile::FiniteDecision => {
            let mut resolved_module = resolved_module;
            crate::commands::prepare_finite_decision_module(&mut resolved_module);
            let expected_program = FiniteDecisionProgramId(Digest::from_bytes(pin_bytes));
            match check_finite_decision_audit_input_bundle_from_module_v1(
                &resolved_module,
                expected_program,
                &plan_limits,
                &decoded_bundle,
                &decode_limits,
            ) {
                Ok(report) => {
                    let rlines: Vec<String> = report
                        .receipt_ids
                        .iter()
                        .enumerate()
                        .map(|(i, r)| format!("receipt[{i}]: verified {}", r.digest().to_hex()))
                        .collect();
                    let rids: Vec<String> = report
                        .receipt_ids
                        .iter()
                        .map(|r| r.digest().to_hex())
                        .collect();
                    (
                        report.context.digest().to_hex(),
                        report.bundle_id.digest().to_hex(),
                        report.final_chain.to_hex(),
                        report.count,
                        rlines,
                        rids,
                    )
                }
                Err(err) => {
                    let msg = format!("{err}");
                    if json {
                        let res = CliResultJson::failure(
                            "verify",
                            Some(profile_str.to_string()),
                            Some(expect_program_hex.to_string()),
                            None,
                            "unknown",
                            vec![format!("verification-error: {err}")],
                        );
                        println!("{}", serde_json::to_string_pretty(&res).unwrap());
                    } else {
                        eprintln!("brix verify: unknown ({msg})");
                    }
                    return EXIT_REJECTED_OR_UNKNOWN;
                }
            }
        }
        VerifyProfile::L3V1 => {
            let expected_program = ProgramIdV1(Digest::from_bytes(pin_bytes));
            match check_l3_audit_input_bundle_from_module_v1(
                &resolved_module,
                expected_program,
                &plan_limits,
                &decoded_bundle,
                &decode_limits,
            ) {
                Ok(report) => {
                    let rlines: Vec<String> = report
                        .receipt_ids
                        .iter()
                        .enumerate()
                        .map(|(i, r)| format!("receipt[{i}]: verified {}", r.digest().to_hex()))
                        .collect();
                    let rids: Vec<String> = report
                        .receipt_ids
                        .iter()
                        .map(|r| r.digest().to_hex())
                        .collect();
                    (
                        report.context.digest().to_hex(),
                        report.bundle_id.digest().to_hex(),
                        report.final_chain.to_hex(),
                        report.count,
                        rlines,
                        rids,
                    )
                }
                Err(err) => {
                    let msg = format!("{err}");
                    if json {
                        let res = CliResultJson::failure(
                            "verify",
                            Some(profile_str.to_string()),
                            Some(expect_program_hex.to_string()),
                            None,
                            "unknown",
                            vec![format!("verification-error: {err}")],
                        );
                        println!("{}", serde_json::to_string_pretty(&res).unwrap());
                    } else {
                        eprintln!("brix verify: unknown ({msg})");
                    }
                    return EXIT_REJECTED_OR_UNKNOWN;
                }
            }
        }
    };

    let artifact = ArtifactJson::bundle(
        Some(bundle_path.display().to_string()),
        bundle_id_hex.clone(),
        final_chain_hex.clone(),
        Some(receipt_ids_hex),
        receipts_count.to_string(),
    );

    // 7. Success output: prints status audit-bundle-verified only after snapshot and every receipt pass.
    if json {
        let res = CliResultJson {
            schema: BRIX_CLI_SCHEMA.to_string(),
            command: "verify".to_string(),
            ok: true,
            profile: Some(profile_str.to_string()),
            program: Some(expect_program_hex.to_string()),
            context: Some(context_hex),
            status: "audit-bundle-verified".to_string(),
            facts: Vec::new(),
            candidates: Vec::new(),
            decision: None,
            artifacts: vec![artifact],
            diagnostics: Vec::new(),
        };
        println!("{}", serde_json::to_string_pretty(&res).unwrap());
    } else {
        for line in receipt_lines {
            println!("{line}");
        }
        println!("status: audit-bundle-verified");
        println!("program: {expect_program_hex}");
        println!("context: {context_hex}");
        println!("bundle_id: {bundle_id_hex}");
        println!("final_chain: {final_chain_hex}");
        println!("receipts: {receipts_count}");
    }

    EXIT_SUCCESS
}

fn hex_to_32_bytes(hex: &str) -> Result<[u8; 32], &'static str> {
    if hex.len() != 64 {
        return Err("expected exactly 64 hex characters");
    }
    let mut bytes = [0u8; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| "invalid non-hex character")?;
    }
    Ok(bytes)
}
