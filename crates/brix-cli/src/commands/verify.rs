//! `brix verify` — verify an audit input bundle against source and external expected program pin (ADR-0026, ADR-0030).

use std::io::Read;
use std::path::{Path, PathBuf};

use brix_canon::Digest;
use brix_lower::audit_bundle::{
    check_finite_decision_audit_input_bundle_from_module_with_inputs_v1,
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

/// Execute `brix verify --expect-program <hex> <file.brix> <bundle> [--profile <finite-decision|l3-v1>] [--input <path>...]`.
pub fn execute_verify(
    expect_program_hex: &str,
    file: &Path,
    bundle_path: &Path,
    profile: VerifyProfile,
    json: bool,
    package_paths: &[PathBuf],
    input_paths: &[PathBuf],
) -> u8 {
    let invocation = match crate::request_evidence::InvocationEvidenceContext::from_environment() {
        Ok(context) => context,
        Err(message) => return crate::pipeline::invocation_evidence_error("verify", message, json),
    };
    execute_verify_with_context(
        expect_program_hex,
        file,
        bundle_path,
        profile,
        json,
        package_paths,
        input_paths,
        &invocation,
    )
}

/// Verify under an explicit per-invocation request evidence context.
#[allow(clippy::too_many_arguments)]
pub fn execute_verify_with_context(
    expect_program_hex: &str,
    file: &Path,
    bundle_path: &Path,
    profile: VerifyProfile,
    json: bool,
    package_paths: &[PathBuf],
    input_paths: &[PathBuf],
    invocation: &crate::request_evidence::InvocationEvidenceContext,
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
                crate::json::emit_result_json(&res);
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
                crate::json::emit_result_json(&res);
            } else {
                eprintln!("brix verify: IO error: {msg}");
            }
            return EXIT_USAGE_OR_IO;
        }
    };

    if bundle_meta.len() > crate::request_audit::MAX_REQUEST_AUDIT_WRAPPER_BYTES as u64 {
        let msg = format!(
            "bundle file '{}' exceeds maximum allowed size ({} bytes > {} limit)",
            bundle_path.display(),
            bundle_meta.len(),
            crate::request_audit::MAX_REQUEST_AUDIT_WRAPPER_BYTES
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
            crate::json::emit_result_json(&res);
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
                crate::json::emit_result_json(&res);
            } else {
                eprintln!("brix verify: IO error: {msg}");
            }
            return EXIT_USAGE_OR_IO;
        }
    };

    let mut bundle_bytes = Vec::new();
    if let Err(err) = bundle_file
        .by_ref()
        .take((crate::request_audit::MAX_REQUEST_AUDIT_WRAPPER_BYTES + 1) as u64)
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
            crate::json::emit_result_json(&res);
        } else {
            eprintln!("brix verify: IO error: {msg}");
        }
        return EXIT_USAGE_OR_IO;
    }

    if bundle_bytes.len() > crate::request_audit::MAX_REQUEST_AUDIT_WRAPPER_BYTES {
        let msg = format!(
            "bundle file '{}' exceeds maximum allowed size ({} bytes > {} limit)",
            bundle_path.display(),
            bundle_bytes.len(),
            crate::request_audit::MAX_REQUEST_AUDIT_WRAPPER_BYTES
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
            crate::json::emit_result_json(&res);
        } else {
            eprintln!("brix verify: IO error: {msg}");
        }
        return EXIT_USAGE_OR_IO;
    }

    // Bundle parsing is delayed until after the source plan and independently
    // supplied request policy are checked. In particular, embedded inputs are
    // not decoded or loaded before wrapper context comparison.

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
                crate::json::emit_result_json(&res);
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
                crate::json::emit_result_json(&res);
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
            let msg = format!("source import error: {err}");
            if json {
                let res = CliResultJson::failure(
                    "verify",
                    Some(profile_str.to_string()),
                    Some(expect_program_hex.to_string()),
                    None,
                    "unknown",
                    vec![format!("source-import-error: {err}")],
                );
                crate::json::emit_result_json(&res);
            } else {
                eprintln!("brix verify: unknown ({msg})");
            }
            return EXIT_REJECTED_OR_UNKNOWN;
        }
    };

    if matches!(profile, VerifyProfile::L3V1) && !input_paths.is_empty() {
        let msg = "--input is not supported for profile 'l3-v1'".to_string();
        if json {
            let res = CliResultJson::failure(
                "verify",
                Some(profile_str.to_string()),
                Some(expect_program_hex.to_string()),
                None,
                "usage-error",
                vec![msg],
            );
            crate::json::emit_result_json(&res);
        } else {
            eprintln!("brix verify: usage error: {msg}");
        }
        return EXIT_USAGE_OR_IO;
    }

    // The policy is bound to this exact source-derived finite-decision plan
    // before wrapper payloads (including input shards) are decoded or loaded.
    let mut evidence_context = crate::request_evidence::RequestEvidenceContext::empty();
    if matches!(profile, VerifyProfile::FiniteDecision) {
        let mut policy_module = resolved_module.clone();
        crate::commands::prepare_finite_decision_module(&mut policy_module);
        let plan = match brix_lower::finite_decision::lower_finite_decision_plan(
            &policy_module,
            brix_lower::FINITE_DECISION_PROFILE,
        ) {
            Ok(plan) => plan,
            Err(err) => {
                let message = format!("source lowering failed: {err}");
                if json {
                    crate::json::emit_result_json(&CliResultJson::failure(
                        "verify",
                        Some(profile_str.to_string()),
                        Some(expect_program_hex.to_string()),
                        None,
                        "unknown",
                        vec![message],
                    ));
                } else {
                    eprintln!("brix verify: unknown ({message})");
                }
                return EXIT_REJECTED_OR_UNKNOWN;
            }
        };
        evidence_context = match crate::pipeline::stage_authorize_evidence(
            "verify",
            Some(profile_str.to_string()),
            Some(expect_program_hex.to_string()),
            &plan,
            invocation,
            json,
        ) {
            Ok(context) => context,
            Err(code) => return code,
        };
    } else if matches!(
        invocation,
        crate::request_evidence::InvocationEvidenceContext::HostPolicy(_)
    ) {
        let message = "request evidence policy is only supported for finite-decision verification";
        if json {
            crate::json::emit_result_json(&CliResultJson::failure(
                "verify",
                Some(profile_str.to_string()),
                Some(expect_program_hex.to_string()),
                None,
                "rejected",
                vec![message.into()],
            ));
        } else {
            eprintln!("brix verify: rejected: {message}");
        }
        return EXIT_REJECTED_OR_UNKNOWN;
    }

    let wrapper_present =
        match crate::request_audit::RequestAuditEvidenceV1::is_wrapper_bytes(&bundle_bytes) {
            Ok(value) => value,
            Err(err) => {
                return report_bundle_error(
                    &err.to_string(),
                    profile_str,
                    expect_program_hex,
                    json,
                    evidence_context.id(),
                );
            }
        };
    let mut _materialized_inputs = None;
    let mut input_paths_for_snapshot = input_paths.to_vec();
    let v1_bundle_bytes = if wrapper_present {
        let (wrapper_context_id, wrapper_context_bytes) =
            match crate::request_audit::RequestAuditEvidenceV1::inspect_context(&bundle_bytes) {
                Ok(pair) => pair,
                Err(err) => {
                    return report_bundle_error(
                        &err.to_string(),
                        profile_str,
                        expect_program_hex,
                        json,
                        evidence_context.id(),
                    )
                }
            };
        let expected_id = evidence_context.id().map(|id| id.digest().to_hex());
        let expected_bytes = evidence_context.canonical_bytes().map(ToOwned::to_owned);
        if wrapper_context_id != expected_id || wrapper_context_bytes != expected_bytes {
            return report_bundle_error(
                "request context does not match the independently authorized policy",
                profile_str,
                expect_program_hex,
                json,
                evidence_context.id(),
            );
        }
        if !input_paths.is_empty() {
            return report_bundle_error(
                "--input cannot be combined with portable request audit inputs",
                profile_str,
                expect_program_hex,
                json,
                evidence_context.id(),
            );
        }
        let wrapper = match crate::request_audit::RequestAuditEvidenceV1::decode(&bundle_bytes) {
            Ok(wrapper) => wrapper,
            Err(err) => {
                return report_bundle_error(
                    &err.to_string(),
                    profile_str,
                    expect_program_hex,
                    json,
                    evidence_context.id(),
                )
            }
        };
        if let Err(err) = wrapper.validate_input_budgets(&evidence_context.read_limits()) {
            return report_bundle_error(
                &err.to_string(),
                profile_str,
                expect_program_hex,
                json,
                evidence_context.id(),
            );
        }
        let materialized =
            match crate::request_audit::MaterializedAuditInputs::create(&wrapper.inputs) {
                Ok(inputs) => inputs,
                Err(err) => {
                    return report_bundle_error(
                        &err.to_string(),
                        profile_str,
                        expect_program_hex,
                        json,
                        evidence_context.id(),
                    )
                }
            };
        input_paths_for_snapshot = materialized.paths().to_vec();
        let bytes = wrapper.v1_bundle;
        _materialized_inputs = Some(materialized);
        bytes
    } else {
        if evidence_context.is_host() {
            return report_bundle_error(
                "unbound V1 audit bundle is not accepted with a host request policy",
                profile_str,
                expect_program_hex,
                json,
                evidence_context.id(),
            );
        }
        bundle_bytes
    };
    let decoded_bundle = match decode_audit_input_bundle_v1(&v1_bundle_bytes, &decode_limits) {
        Ok(bundle) => bundle,
        Err(err) => {
            return report_bundle_error(
                &format!("bundle-decode-error: {err:?}"),
                profile_str,
                expect_program_hex,
                json,
                evidence_context.id(),
            )
        }
    };
    let request_context_id = evidence_context.id();

    // 6. Call the correct source/module verifier.
    let plan_limits = PlanLimitsV1::generous();

    let (
        context_hex,
        input_snapshot_hex,
        bundle_id_hex,
        final_chain_hex,
        receipts_count,
        receipt_lines,
        receipt_ids_hex,
    ) = match profile {
        VerifyProfile::FiniteDecision => {
            let snapshot = match crate::commands::load_cli_input_snapshot_with_context(
                &input_paths_for_snapshot,
                &evidence_context,
            ) {
                Ok(s) => s,
                Err(err) => {
                    if json {
                        let res = CliResultJson::failure(
                            "verify",
                            Some(profile_str.to_string()),
                            Some(expect_program_hex.to_string()),
                            None,
                            err.status(),
                            vec![err.diagnostic()],
                        );
                        crate::json::emit_result_json_with_request_context(
                            &res,
                            request_context_id,
                        );
                    } else {
                        eprintln!("{}", err.render_human("verify"));
                    }
                    return err.exit_code();
                }
            };
            let input_snapshot_hex = if !snapshot.is_empty() {
                Some(snapshot.id().0.to_hex())
            } else {
                None
            };
            let mut resolved_module = resolved_module;
            crate::commands::prepare_finite_decision_module(&mut resolved_module);
            let expected_program = FiniteDecisionProgramId(Digest::from_bytes(pin_bytes));
            match check_finite_decision_audit_input_bundle_from_module_with_inputs_v1(
                &resolved_module,
                expected_program,
                &plan_limits,
                &decoded_bundle,
                &decode_limits,
                &snapshot,
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
                        input_snapshot_hex,
                        report.bundle_id.digest().to_hex(),
                        report.final_chain.to_hex(),
                        report.count,
                        rlines,
                        rids,
                    )
                }
                Err(err) => {
                    let (status, diag) = match &err {
                        brix_lower::audit_bundle::SourceBundleError::InputValidation(iv) => {
                            let cli_err = crate::commands::CliInputError::from(iv.clone());
                            (cli_err.status(), cli_err.diagnostic())
                        }
                        other => ("unknown", format!("verification-error: {other}")),
                    };
                    if json {
                        let res = CliResultJson::failure(
                            "verify",
                            Some(profile_str.to_string()),
                            Some(expect_program_hex.to_string()),
                            None,
                            status,
                            vec![diag.clone()],
                        )
                        .with_inputs(input_snapshot_hex, None);
                        crate::json::emit_result_json_with_request_context(
                            &res,
                            request_context_id,
                        );
                    } else {
                        match &err {
                            brix_lower::audit_bundle::SourceBundleError::InputValidation(iv) => {
                                let cli_err = crate::commands::CliInputError::from(iv.clone());
                                eprintln!("{}", cli_err.render_human("verify"));
                            }
                            _ => {
                                eprintln!("brix verify: unknown ({diag})");
                            }
                        }
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
                        None,
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
                        crate::json::emit_result_json(&res);
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
            input_snapshot: input_snapshot_hex,
            status: "audit-bundle-verified".to_string(),
            inputs: None,
            facts: Vec::new(),
            candidates: Vec::new(),
            decision: None,
            artifacts: vec![artifact],
            diagnostics: Vec::new(),
            bindings: None,
            explanation: None,
            locations: None,
            shows: None,
            commits: None,
            entity_decisions: None,
        };
        crate::json::emit_result_json_with_request_context(&res, request_context_id);
    } else {
        for line in receipt_lines {
            println!("{line}");
        }
        println!("status: audit-bundle-verified");
        println!("program: {expect_program_hex}");
        println!("context: {context_hex}");
        if let Some(snap) = &input_snapshot_hex {
            println!("input-snapshot: {snap}");
        }
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

fn report_bundle_error(
    message: &str,
    profile: &str,
    expect_program: &str,
    json: bool,
    request_context: Option<brix_semantic::ContextId>,
) -> u8 {
    if json {
        let result = CliResultJson::failure(
            "verify",
            Some(profile.to_string()),
            Some(expect_program.to_string()),
            None,
            "unknown",
            vec![format!("request-audit-bundle-error: {message}")],
        );
        crate::json::emit_result_json_with_request_context(&result, request_context);
    } else {
        eprintln!("brix verify: unknown (request-audit-bundle-error: {message})");
    }
    EXIT_REJECTED_OR_UNKNOWN
}
