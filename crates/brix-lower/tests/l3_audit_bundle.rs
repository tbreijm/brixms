//! Tests for source-derived audit bundle integration (ADR-0026).

use std::path::{Path, PathBuf};

use brix_canon::{Canonical, Digest, Domain};
use brix_lower::l3_v2::L3ValueV2;
use brix_lower::{
    canonicalize_input_shards, check_finite_decision_audit_input_bundle_from_source_v1,
    check_finite_decision_audit_input_bundle_from_source_with_inputs_v1,
    check_l3_audit_input_bundle_from_source_v1, decode_audit_input_bundle_v1, decode_input_shard,
    finite_decision_program_id, lower_finite_decision_plan, lower_l3_plan,
    produce_finite_decision_audit_input_bundle_v1, produce_l3_audit_input_bundle_v1, program_id,
    run_l3_plan, AuditDecodeLimits, BundleCheckError, ContextId, FiniteDecisionPlan,
    FiniteDecisionProgramId, FiniteDecisionRuntime, FiniteDecisionStop, InputLimits, L3AdmChoice,
    PlanLimitsV1, ProgramIdV1, SettlementAuditInputBundleV1, SettlementStopV1, SourceBundleError,
    SourceBundleProducerError, FINITE_DECISION_PROFILE, L3_PROFILE_MARKER_V1,
};
use brix_syntax::{parse, ParseLimits};
use soc_core::history::History;
use soc_core::saturate::SaturationBudget;

const L3_SRC: &str = "rule a() = 1\nrule b() = 2\n";

const FD_SELECTED_SRC: &str = r#"
config Arrangement = A | B

rule base() = 1

propose opt_a(base) priority 10 when base == 1 = A
propose opt_b(base) priority 20 when base == 1 = B

commit pick from (opt_a, opt_b)
"#;

const FD_QUIESCENT_SRC: &str = r#"
config Arrangement = A | B

rule base() = 2

propose opt_a(base) priority 10 when base == 1 = A

commit pick from (opt_a)
"#;

const FD_FAULT_SRC: &str = r#"
config Arrangement = A | B

rule base() = 1

propose opt_bad(base) priority 10 when 42 = A

commit pick from (opt_bad)
"#;

fn generous_budget() -> SaturationBudget {
    SaturationBudget::uniform(1_000)
}

fn fd_plan(source: &str) -> FiniteDecisionPlan {
    let module = parse(source).expect("finite-decision fixture parses");
    lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
        .expect("finite-decision fixture lowers")
}

// ---------------------------------------------------------------------------
// 1. Honest cross-instance produce/verify for both profiles
// ---------------------------------------------------------------------------

#[test]
fn honest_cross_instance_produce_and_verify_l3_v1() {
    let module = parse(L3_SRC).expect("source parses");
    let plan = lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
        .expect("source lowers");
    let expected_prog = program_id(&plan);

    let report = run_l3_plan(&plan, L3AdmChoice::Compiled, generous_budget());
    assert_eq!(report.journal.len(), 2);

    let bundle =
        produce_l3_audit_input_bundle_v1(&report, &report.run).expect("bundle production succeeds");

    let bundle_bytes = bundle.canon_bytes();
    let decoded_bundle = decode_audit_input_bundle_v1(&bundle_bytes, &AuditDecodeLimits::strict())
        .expect("bundle decoding succeeds");

    let verification_report = check_l3_audit_input_bundle_from_source_v1(
        L3_SRC.as_bytes(),
        expected_prog,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &decoded_bundle,
        &AuditDecodeLimits::strict(),
    )
    .expect("verification from source succeeds");

    assert_eq!(verification_report.program, expected_prog);
    assert_eq!(verification_report.context, report.context);
    assert_eq!(verification_report.bundle_id, bundle.id());
    assert_eq!(verification_report.final_chain, bundle.final_chain_digest);
    assert_eq!(verification_report.receipt_ids.len(), 2);
    assert_eq!(verification_report.count, 2);
    assert_eq!(verification_report.status(), "audit-bundle-verified");
}

#[test]
fn honest_cross_instance_produce_and_verify_finite_decision_selected() {
    let plan = fd_plan(FD_SELECTED_SRC);
    let expected_prog = finite_decision_program_id(&plan);

    let runtime = FiniteDecisionRuntime::build(&plan).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());
    assert_eq!(run.journal.len(), 1);

    let bundle = produce_finite_decision_audit_input_bundle_v1(&runtime, &run)
        .expect("finite-decision bundle production succeeds");

    let bundle_bytes = bundle.canon_bytes();
    let decoded_bundle = decode_audit_input_bundle_v1(&bundle_bytes, &AuditDecodeLimits::strict())
        .expect("bundle decoding succeeds");

    let verification_report = check_finite_decision_audit_input_bundle_from_source_v1(
        FD_SELECTED_SRC.as_bytes(),
        expected_prog,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &decoded_bundle,
        &AuditDecodeLimits::strict(),
    )
    .expect("verification from source succeeds");

    assert_eq!(verification_report.program, expected_prog);
    assert_eq!(verification_report.context, runtime.context);
    assert_eq!(verification_report.bundle_id, bundle.id());
    assert_eq!(verification_report.final_chain, bundle.final_chain_digest);
    assert_eq!(verification_report.receipt_ids.len(), 1);
    assert_eq!(verification_report.count, 1);
    assert_eq!(verification_report.status(), "audit-bundle-verified");
}

#[test]
fn honest_cross_instance_produce_and_verify_finite_decision_quiescent() {
    let plan = fd_plan(FD_QUIESCENT_SRC);
    let expected_prog = finite_decision_program_id(&plan);

    let runtime = FiniteDecisionRuntime::build(&plan).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_quiescent());
    assert_eq!(run.journal.len(), 0);

    let bundle = produce_finite_decision_audit_input_bundle_v1(&runtime, &run)
        .expect("quiescent bundle production succeeds");

    let verification_report = check_finite_decision_audit_input_bundle_from_source_v1(
        FD_QUIESCENT_SRC.as_bytes(),
        expected_prog,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &bundle,
        &AuditDecodeLimits::strict(),
    )
    .expect("verification succeeds");

    assert_eq!(verification_report.program, expected_prog);
    assert_eq!(verification_report.context, runtime.context);
    assert_eq!(verification_report.bundle_id, bundle.id());
    assert_eq!(verification_report.final_chain, History::empty().digest());
    assert_eq!(verification_report.receipt_ids.len(), 0);
    assert_eq!(verification_report.count, 0);
    assert_eq!(verification_report.status(), "audit-bundle-verified");
}

// ---------------------------------------------------------------------------
// 2. Target / Source / Context mismatch
// ---------------------------------------------------------------------------

#[test]
fn target_mismatch_is_rejected_for_both_profiles() {
    // L3 v1
    let module = parse(L3_SRC).expect("source parses");
    let plan = lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
        .expect("source lowers");
    let true_prog = program_id(&plan);
    let foreign_prog = ProgramIdV1(Digest::of(Domain::Value, b"different-program-id"));
    let report = run_l3_plan(&plan, L3AdmChoice::Compiled, generous_budget());
    let bundle = produce_l3_audit_input_bundle_v1(&report, &report.run).expect("bundle");

    match check_l3_audit_input_bundle_from_source_v1(
        L3_SRC.as_bytes(),
        foreign_prog,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &bundle,
        &AuditDecodeLimits::strict(),
    ) {
        Err(SourceBundleError::ProgramMismatch { expected, derived }) => {
            assert_eq!(expected, foreign_prog);
            assert_eq!(derived, true_prog);
        }
        other => panic!("expected ProgramMismatch, got {other:?}"),
    }

    // Finite-decision
    let fd_p = fd_plan(FD_SELECTED_SRC);
    let true_fd_prog = finite_decision_program_id(&fd_p);
    let foreign_fd_prog =
        FiniteDecisionProgramId(Digest::of(Domain::Value, b"different-fd-prog-id"));
    let runtime = FiniteDecisionRuntime::build(&fd_p).expect("runtime builds");
    let run = runtime.run();
    let fd_bundle = produce_finite_decision_audit_input_bundle_v1(&runtime, &run).expect("bundle");

    match check_finite_decision_audit_input_bundle_from_source_v1(
        FD_SELECTED_SRC.as_bytes(),
        foreign_fd_prog,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &fd_bundle,
        &AuditDecodeLimits::strict(),
    ) {
        Err(SourceBundleError::ProgramMismatch { expected, derived }) => {
            assert_eq!(expected, foreign_fd_prog);
            assert_eq!(derived, true_fd_prog);
        }
        other => panic!("expected ProgramMismatch, got {other:?}"),
    }
}

#[test]
fn source_mismatch_is_rejected_for_both_profiles() {
    // L3 v1: expected program is from L3_SRC, but source passed is different.
    let module_a = parse(L3_SRC).expect("source a");
    let plan_a = lower_l3_plan(&module_a, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
        .expect("lowers a");
    let prog_a = program_id(&plan_a);
    let report_a = run_l3_plan(&plan_a, L3AdmChoice::Compiled, generous_budget());
    let bundle_a = produce_l3_audit_input_bundle_v1(&report_a, &report_a.run).expect("bundle a");

    let other_src = "rule a() = 1\nrule b() = 999\n";
    let module_b = parse(other_src).expect("source b");
    let plan_b = lower_l3_plan(&module_b, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
        .expect("lowers b");
    let prog_b = program_id(&plan_b);

    match check_l3_audit_input_bundle_from_source_v1(
        other_src.as_bytes(),
        prog_a,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &bundle_a,
        &AuditDecodeLimits::strict(),
    ) {
        Err(SourceBundleError::ProgramMismatch { expected, derived }) => {
            assert_eq!(expected, prog_a);
            assert_eq!(derived, prog_b);
        }
        other => panic!("expected ProgramMismatch, got {other:?}"),
    }

    // Finite-decision
    let plan_fd_a = fd_plan(FD_SELECTED_SRC);
    let prog_fd_a = finite_decision_program_id(&plan_fd_a);
    let runtime_a = FiniteDecisionRuntime::build(&plan_fd_a).expect("runtime builds");
    let run_a = runtime_a.run();
    let bundle_fd_a =
        produce_finite_decision_audit_input_bundle_v1(&runtime_a, &run_a).expect("bundle");

    let plan_fd_b = fd_plan(FD_QUIESCENT_SRC);
    let prog_fd_b = finite_decision_program_id(&plan_fd_b);

    match check_finite_decision_audit_input_bundle_from_source_v1(
        FD_QUIESCENT_SRC.as_bytes(),
        prog_fd_a,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &bundle_fd_a,
        &AuditDecodeLimits::strict(),
    ) {
        Err(SourceBundleError::ProgramMismatch { expected, derived }) => {
            assert_eq!(expected, prog_fd_a);
            assert_eq!(derived, prog_fd_b);
        }
        other => panic!("expected ProgramMismatch, got {other:?}"),
    }
}

#[test]
fn context_mismatch_is_rejected_for_both_profiles() {
    // L3 v1
    let module = parse(L3_SRC).expect("source");
    let plan =
        lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous()).expect("lowers");
    let prog = program_id(&plan);
    let report = run_l3_plan(&plan, L3AdmChoice::Compiled, generous_budget());
    let mut bundle = produce_l3_audit_input_bundle_v1(&report, &report.run).expect("bundle");

    let wrong_context = ContextId::from_canon(b"foreign-context-id");
    bundle.context = wrong_context;

    match check_l3_audit_input_bundle_from_source_v1(
        L3_SRC.as_bytes(),
        prog,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &bundle,
        &AuditDecodeLimits::strict(),
    ) {
        Err(SourceBundleError::ContextMismatch { expected, found }) => {
            assert_eq!(expected, report.context);
            assert_eq!(found, wrong_context);
        }
        other => panic!("expected ContextMismatch, got {other:?}"),
    }

    // Finite-decision
    let fd_p = fd_plan(FD_SELECTED_SRC);
    let fd_prog = finite_decision_program_id(&fd_p);
    let runtime = FiniteDecisionRuntime::build(&fd_p).expect("runtime builds");
    let run = runtime.run();
    let mut fd_bundle =
        produce_finite_decision_audit_input_bundle_v1(&runtime, &run).expect("bundle");

    fd_bundle.context = wrong_context;

    match check_finite_decision_audit_input_bundle_from_source_v1(
        FD_SELECTED_SRC.as_bytes(),
        fd_prog,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &fd_bundle,
        &AuditDecodeLimits::strict(),
    ) {
        Err(SourceBundleError::ContextMismatch { expected, found }) => {
            assert_eq!(expected, runtime.context);
            assert_eq!(found, wrong_context);
        }
        other => panic!("expected ContextMismatch, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 3. Receipt failure propagation
// ---------------------------------------------------------------------------

#[test]
fn receipt_failure_propagation() {
    let module = parse(L3_SRC).expect("source");
    let plan =
        lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous()).expect("lowers");
    let prog = program_id(&plan);
    let report = run_l3_plan(&plan, L3AdmChoice::Compiled, generous_budget());
    let bundle = produce_l3_audit_input_bundle_v1(&report, &report.run).expect("bundle");

    // Case A: Corrupted receipt bytes
    let mut tampered_receipt_bundle = bundle.clone();
    tampered_receipt_bundle.entries[0].receipt_bytes[10] ^= 0xff;

    match check_l3_audit_input_bundle_from_source_v1(
        L3_SRC.as_bytes(),
        prog,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &tampered_receipt_bundle,
        &AuditDecodeLimits::strict(),
    ) {
        Err(SourceBundleError::BundleCheck(BundleCheckError::Receipt { ordinal, error })) => {
            assert_eq!(ordinal, 0);
            assert!(matches!(
                error,
                soc_core::ReceiptError::FieldMismatch { .. }
                    | soc_core::ReceiptError::BadMarker
                    | soc_core::ReceiptError::MalformedReceipt(_)
                    | soc_core::ReceiptError::TrailingBytes
            ));
        }
        other => panic!("expected BundleCheck(Receipt), got {other:?}"),
    }

    // Case B: Tampered prefix digest
    let mut tampered_snapshot_bundle = bundle.clone();
    tampered_snapshot_bundle.entries[1].prefix_digest = Digest::of(Domain::Value, b"forged-prefix");

    match check_l3_audit_input_bundle_from_source_v1(
        L3_SRC.as_bytes(),
        prog,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &tampered_snapshot_bundle,
        &AuditDecodeLimits::strict(),
    ) {
        Err(SourceBundleError::BundleCheck(BundleCheckError::SnapshotValidation(
            soc_core::audit_bundle::SnapshotValidationError::PrefixDigestMismatch {
                ordinal, ..
            },
        ))) => {
            assert_eq!(ordinal, 1);
        }
        other => panic!("expected BundleCheck(SnapshotValidation), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 4. Unknown producer refusal and mismatch checks
// ---------------------------------------------------------------------------

#[test]
fn unknown_producer_refusal_for_both_profiles() {
    // L3 v1: run with budget 0 halts with Unknown
    let module = parse(L3_SRC).expect("source");
    let plan =
        lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous()).expect("lowers");
    let report_unknown = run_l3_plan(&plan, L3AdmChoice::Compiled, SaturationBudget::uniform(0));
    assert!(matches!(
        report_unknown.run.stop,
        SettlementStopV1::Unknown { .. }
    ));

    let err = produce_l3_audit_input_bundle_v1(&report_unknown, &report_unknown.run).unwrap_err();
    assert_eq!(err, SourceBundleProducerError::UnknownRun);

    // Finite-decision: program with type fault halts with Unknown
    let plan_fault = fd_plan(FD_FAULT_SRC);
    let runtime_fault = FiniteDecisionRuntime::build(&plan_fault).expect("runtime builds");
    let run_fault = runtime_fault.run();
    assert!(run_fault.is_unknown());
    assert!(matches!(run_fault.stop, FiniteDecisionStop::Unknown(_)));

    let fd_err =
        produce_finite_decision_audit_input_bundle_v1(&runtime_fault, &run_fault).unwrap_err();
    assert_eq!(fd_err, SourceBundleProducerError::UnknownRun);
}

#[test]
fn producer_refuses_mismatched_runtime_and_run() {
    // L3 v1: cross-pair report A with run B
    let module_a = parse(L3_SRC).expect("source a");
    let plan_a = lower_l3_plan(&module_a, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
        .expect("lowers a");
    let report_a = run_l3_plan(&plan_a, L3AdmChoice::Compiled, generous_budget());

    let other_src = "rule a() = 1\nrule b() = 999\n";
    let module_b = parse(other_src).expect("source b");
    let plan_b = lower_l3_plan(&module_b, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
        .expect("lowers b");
    let report_b = run_l3_plan(&plan_b, L3AdmChoice::Compiled, generous_budget());

    let err_l3 = produce_l3_audit_input_bundle_v1(&report_a, &report_b.run).unwrap_err();
    assert!(matches!(err_l3, SourceBundleProducerError::RunMismatch(_)));

    // Finite-decision: cross-pair runtime A with run B
    let plan_fd_a = fd_plan(FD_SELECTED_SRC);
    let runtime_fd_a = FiniteDecisionRuntime::build(&plan_fd_a).expect("runtime builds");

    let plan_fd_b = fd_plan(FD_QUIESCENT_SRC);
    let runtime_fd_b = FiniteDecisionRuntime::build(&plan_fd_b).expect("runtime builds");
    let run_fd_b = runtime_fd_b.run();

    let err_fd =
        produce_finite_decision_audit_input_bundle_v1(&runtime_fd_a, &run_fd_b).unwrap_err();
    assert!(matches!(err_fd, SourceBundleProducerError::RunMismatch(_)));
}

#[test]
fn producer_refuses_tampered_run_bound_inputs() {
    let source = r#"
input limit: Int

config Arrangement = A | B

rule base() = limit

propose opt_a(base) priority 10 when base == 100 = A
propose opt_b(base) priority 20 when base == 100 = B

commit pick from (opt_a, opt_b)
"#;
    let p = fd_plan(source);
    let json = r#"{
        "schema": "brix.input@1",
        "values": {
            "limit": {"type": "int", "value": "100"}
        }
    }"#;
    let shard =
        decode_input_shard(json.as_bytes(), &InputLimits::default()).expect("shard decodes");
    let snapshot =
        canonicalize_input_shards(vec![shard], &InputLimits::default()).expect("snapshot forms");

    let runtime = FiniteDecisionRuntime::build_with_inputs(&p, &snapshot).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());

    // Honest run produces bundle successfully
    let honest_bundle = produce_finite_decision_audit_input_bundle_v1(&runtime, &run);
    assert!(
        honest_bundle.is_ok(),
        "honest bundle production must succeed"
    );

    // Case 1: Tampered input value in public run report
    let mut tampered_run_val = run.clone();
    tampered_run_val.inputs[0].value = L3ValueV2::Int(999);
    let err_val =
        produce_finite_decision_audit_input_bundle_v1(&runtime, &tampered_run_val).unwrap_err();
    match err_val {
        SourceBundleProducerError::RunMismatch(msg) => {
            assert!(
                msg.contains("bound inputs mismatch"),
                "expected bound inputs mismatch message, got: {msg}"
            );
        }
        other => panic!("expected RunMismatch, got: {other:?}"),
    }

    // Case 2: Tampered input list in public run report (e.g. cleared inputs)
    let mut tampered_run_empty = run.clone();
    tampered_run_empty.inputs.clear();
    let err_empty =
        produce_finite_decision_audit_input_bundle_v1(&runtime, &tampered_run_empty).unwrap_err();
    match err_empty {
        SourceBundleProducerError::RunMismatch(msg) => {
            assert!(
                msg.contains("bound inputs mismatch"),
                "expected bound inputs mismatch message, got: {msg}"
            );
        }
        other => panic!("expected RunMismatch, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 5. Source size checked before UTF-8 validation
// ---------------------------------------------------------------------------

#[test]
fn oversized_source_is_refused_before_utf8_validation() {
    let dummy_prog = ProgramIdV1(Digest::of(Domain::Value, b"dummy"));
    let dummy_fd_prog = FiniteDecisionProgramId(Digest::of(Domain::Value, b"dummy"));
    let dummy_bundle = SettlementAuditInputBundleV1 {
        context: ContextId::from_canon(b"dummy"),
        entries: Vec::new(),
        final_chain_digest: History::empty().digest(),
    };

    let limits = ParseLimits {
        max_source_bytes: 16,
        ..ParseLimits::strict()
    };
    // 64 bytes of non-UTF-8 data
    let hostile = vec![0xffu8; 64];

    match check_l3_audit_input_bundle_from_source_v1(
        &hostile,
        dummy_prog,
        limits,
        &PlanLimitsV1::generous(),
        &dummy_bundle,
        &AuditDecodeLimits::strict(),
    ) {
        Err(SourceBundleError::SourceTooLarge { limit, found }) => {
            assert_eq!(limit, 16);
            assert_eq!(found, 64);
        }
        Err(SourceBundleError::InvalidUtf8) => {
            panic!("size bound must trigger before UTF-8 check")
        }
        other => panic!("expected SourceTooLarge, got {other:?}"),
    }

    match check_finite_decision_audit_input_bundle_from_source_v1(
        &hostile,
        dummy_fd_prog,
        limits,
        &PlanLimitsV1::generous(),
        &dummy_bundle,
        &AuditDecodeLimits::strict(),
    ) {
        Err(SourceBundleError::SourceTooLarge { limit, found }) => {
            assert_eq!(limit, 16);
            assert_eq!(found, 64);
        }
        Err(SourceBundleError::InvalidUtf8) => {
            panic!("size bound must trigger before UTF-8 check")
        }
        other => panic!("expected SourceTooLarge, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 6. Unchanged v1 vectors
// ---------------------------------------------------------------------------

fn vector_file_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors/settlement_audit_input_bundle_v1.json")
}

fn from_hex(hex: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for i in (0..hex.len()).step_by(2) {
        let byte = u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex");
        bytes.push(byte);
    }
    bytes
}

#[test]
fn settlement_audit_input_bundle_v1_vectors_are_unchanged() {
    let path = vector_file_path();
    let content = std::fs::read_to_string(&path).expect("vector file must exist");

    assert!(content.contains("\"format\": \"brix.soc.SettlementAuditInputBundleV1\""));
    assert!(content.contains("\"name\": \"two_link_fixture_chain_bundle\""));

    // Extract fields from the known vector
    let context_prefix = "\"context_id\": \"";
    let context_start =
        content.find(context_prefix).expect("has context_id") + context_prefix.len();
    let context_hex = &content[context_start..context_start + 64];

    let final_chain_prefix = "\"final_chain_digest\": \"";
    let final_chain_start = content
        .find(final_chain_prefix)
        .expect("has final_chain_digest")
        + final_chain_prefix.len();
    let final_chain_hex = &content[final_chain_start..final_chain_start + 64];

    let bundle_id_prefix = "\"bundle_id\": \"";
    let bundle_id_start =
        content.find(bundle_id_prefix).expect("has bundle_id") + bundle_id_prefix.len();
    let bundle_id_hex = &content[bundle_id_start..bundle_id_start + 64];

    let canon_hex_prefix = "\"canon_hex\": \"";
    let canon_hex_start =
        content.find(canon_hex_prefix).expect("has canon_hex") + canon_hex_prefix.len();
    let canon_hex_end =
        content[canon_hex_start..].find('"').expect("closing quote") + canon_hex_start;
    let canon_hex = &content[canon_hex_start..canon_hex_end];

    let raw_bytes = from_hex(canon_hex);
    let decoded = decode_audit_input_bundle_v1(&raw_bytes, &AuditDecodeLimits::strict())
        .expect("vector bytes decode cleanly");

    assert_eq!(decoded.context.digest().to_hex(), context_hex);
    assert_eq!(decoded.final_chain_digest.to_hex(), final_chain_hex);
    assert_eq!(decoded.id().to_hex(), bundle_id_hex);
    assert_eq!(decoded.entries.len(), 1);
}

// ---------------------------------------------------------------------------
// 7. Input-aware finite-decision bundle verification tests
// ---------------------------------------------------------------------------

const FD_INPUT_SRC: &str = r#"
input limit: Int
input enabled: Bool

config Arrangement = A | B

rule base() = limit
rule is_enabled() = enabled

propose opt_a(base) priority 10 when base == 100 = A
propose opt_b(is_enabled) priority 20 when is_enabled = B

commit pick from (opt_a, opt_b)
"#;

fn make_input_snapshot(limit: i64, enabled: bool) -> brix_lower::InputSnapshot {
    let json = format!(
        r#"{{
        "schema": "brix.input@1",
        "values": {{
            "limit": {{"type": "int", "value": "{limit}"}},
            "enabled": {{"type": "bool", "value": {enabled}}}
        }}
    }}"#
    );
    let shard =
        decode_input_shard(json.as_bytes(), &InputLimits::default()).expect("shard decodes");
    canonicalize_input_shards(vec![shard], &InputLimits::default()).expect("snapshot canonicalizes")
}

#[test]
fn honest_cross_instance_produce_and_verify_finite_decision_with_inputs() {
    let plan = fd_plan(FD_INPUT_SRC);
    let snapshot = make_input_snapshot(100, true);

    let runtime =
        FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());

    let bundle =
        produce_finite_decision_audit_input_bundle_v1(&runtime, &run).expect("bundle produced");
    let prog_id = finite_decision_program_id(&plan);

    let report = check_finite_decision_audit_input_bundle_from_source_with_inputs_v1(
        FD_INPUT_SRC.as_bytes(),
        prog_id,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &bundle,
        &AuditDecodeLimits::strict(),
        &snapshot,
    )
    .expect("verification succeeds with matching snapshot");

    assert_eq!(report.program, prog_id);
    assert_eq!(report.context, runtime.context);
    assert_eq!(report.bundle_id, bundle.id());
    assert_eq!(report.count, 1);
}

#[test]
fn verify_finite_decision_with_inputs_fails_on_missing_or_tampered_snapshot() {
    let plan = fd_plan(FD_INPUT_SRC);
    let snapshot_honest = make_input_snapshot(100, true);

    let runtime =
        FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot_honest).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());

    let bundle =
        produce_finite_decision_audit_input_bundle_v1(&runtime, &run).expect("bundle produced");
    let prog_id = finite_decision_program_id(&plan);

    // 1. Missing input snapshot (empty snapshot against program declaring inputs)
    let empty_snapshot = brix_lower::InputSnapshot::empty();
    let err_missing = check_finite_decision_audit_input_bundle_from_source_with_inputs_v1(
        FD_INPUT_SRC.as_bytes(),
        prog_id,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &bundle,
        &AuditDecodeLimits::strict(),
        &empty_snapshot,
    )
    .unwrap_err();

    assert!(
        matches!(
            err_missing,
            SourceBundleError::InputValidation(
                brix_lower::InputValidationError::MissingInput { .. }
            )
        ),
        "expected InputValidation(MissingInput), got: {err_missing:?}"
    );

    // 2. Changed input value (limit=200 instead of 100): ProgramId holds constant, ContextId changes, fails ContextMismatch
    let snapshot_tampered = make_input_snapshot(200, true);
    let err_mismatch = check_finite_decision_audit_input_bundle_from_source_with_inputs_v1(
        FD_INPUT_SRC.as_bytes(),
        prog_id,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &bundle,
        &AuditDecodeLimits::strict(),
        &snapshot_tampered,
    )
    .unwrap_err();

    assert!(
        matches!(err_mismatch, SourceBundleError::ContextMismatch { .. }),
        "expected ContextMismatch due to changed input value, got: {err_mismatch:?}"
    );

    // 3. Extra undeclared input supplied
    let json_extra = r#"{
        "schema": "brix.input@1",
        "values": {
            "limit": {"type": "int", "value": "100"},
            "enabled": {"type": "bool", "value": true},
            "unrelated": {"type": "string", "value": "extra"}
        }
    }"#;
    let shard_extra = decode_input_shard(json_extra.as_bytes(), &InputLimits::default()).unwrap();
    let snapshot_extra =
        canonicalize_input_shards(vec![shard_extra], &InputLimits::default()).unwrap();

    let err_extra = check_finite_decision_audit_input_bundle_from_source_with_inputs_v1(
        FD_INPUT_SRC.as_bytes(),
        prog_id,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &bundle,
        &AuditDecodeLimits::strict(),
        &snapshot_extra,
    )
    .unwrap_err();

    assert!(
        matches!(
            err_extra,
            SourceBundleError::InputValidation(
                brix_lower::InputValidationError::UndeclaredInput { .. }
            )
        ),
        "expected InputValidation(UndeclaredInput), got: {err_extra:?}"
    );
}
