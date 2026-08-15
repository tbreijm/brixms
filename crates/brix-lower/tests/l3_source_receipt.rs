//! ADR-0022 gates: re-deriving the expected L3 audit environment from source.
//!
//! The positive case is unremarkable — the same source produces the same
//! program, table and manifest, so an honest receipt validates. The gates that
//! carry the ADR are the negatives:
//!
//! - a **substituted program** is refused by `ProgramMismatch` and never falls
//!   back to the receipt's own manifest (D5);
//! - the entry point takes **no expected registry or semantics parameter**, so
//!   a caller cannot supply the expectation the receipt is checked against
//!   (D3) — this is what closes ADR-0020 residual 2 for this path;
//! - hostile source is refused by **local resource policy** before it can
//!   exhaust the verifier (D6), and that refusal is typed, not a panic.

use brix_lower::l3_regime::build_l3_transition_table;
use brix_lower::SourceReceiptError;
use brix_lower::{
    check_l3_audit_receipt_from_source_v1, l3_generator_registry, l3_generator_semantics,
    lower_l3_plan, program_id, run_l3_plan_with_interner, L3AdmChoice, PlanLimitsV1,
    L3_PROFILE_MARKER_V1,
};
use brix_syntax::{parse, ParseLimits};
use soc_core::audit::{audit_step, AuditResult};
use soc_core::intern::Interner;

const SRC: &str = "rule a() = 1\nrule b() = 2\n";

/// Produce a real run over `SRC` and take the first committed step plus the
/// receipt its audit issued.
fn fixture() -> (
    soc_core::journal::CommittedStep,
    brix_semantic::ContextId,
    soc_core::audit_receipt::SettlementAuditReceiptV1,
    brix_lower::ProgramIdV1,
) {
    let module = parse(SRC).expect("fixture source parses");
    let plan = lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
        .expect("fixture source lowers");
    let program = program_id(&plan);

    let mut interner = Interner::new();
    let report = run_l3_plan_with_interner(
        &mut interner,
        &plan,
        L3AdmChoice::Compiled,
        soc_core::saturate::SaturationBudget::uniform(8),
    );

    let step = report
        .journal
        .steps()
        .first()
        .expect("at least one committed step")
        .clone();

    let registry = l3_generator_registry(&report.table);
    let semantics = l3_generator_semantics(&report.table);
    let receipt = match audit_step(&step, report.context, &registry, &semantics) {
        AuditResult::Audited(a) => a.receipt.clone(),
        AuditResult::Unknown(r) => panic!("fixture audit must succeed, got {r}"),
    };

    (step, report.context, receipt, program)
}

// ---------------------------------------------------------------------------
// Positive
// ---------------------------------------------------------------------------

#[test]
fn source_re_derives_the_expected_environment_and_validates_the_receipt() {
    let (step, context, receipt, program) = fixture();

    let id = check_l3_audit_receipt_from_source_v1(
        SRC.as_bytes(),
        program,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &receipt,
        &step,
        context,
    )
    .expect("source re-derivation validates the honest receipt");

    assert_eq!(id, receipt.id());
}

/// Equivalent spellings are the same program: source is a *witness* for the
/// already-frozen `ProgramIdV1`, not another identity-bearing artifact
/// (ADR-0022 §4).
#[test]
fn whitespace_and_comment_variation_re_derive_the_same_program() {
    let (step, context, receipt, program) = fixture();
    let respelled = "rule a() = 1\n\n\nrule b()   =   2\n";

    let id = check_l3_audit_receipt_from_source_v1(
        respelled.as_bytes(),
        program,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &receipt,
        &step,
        context,
    )
    .expect("an equivalent spelling re-derives the same program");
    assert_eq!(id, receipt.id());
}

// ---------------------------------------------------------------------------
// Negative — the gates that carry the ADR
// ---------------------------------------------------------------------------

/// **D5.** Different source is a different program, and the mismatch is
/// reported as such — never normalized away, never falling back to the
/// receipt's own manifest id.
#[test]
fn a_substituted_program_is_refused_by_program_mismatch() {
    let (step, context, receipt, program) = fixture();
    let other = "rule a() = 1\nrule b() = 99\n";

    match check_l3_audit_receipt_from_source_v1(
        other.as_bytes(),
        program,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &receipt,
        &step,
        context,
    ) {
        Err(SourceReceiptError::ProgramMismatch { expected, derived }) => {
            assert_eq!(expected, program);
            assert_ne!(derived, program);
        }
        other => panic!("substituted source must be refused by name, got {other:?}"),
    }
}

/// **D6.** Hostile source is refused by local resource policy, as a typed
/// error rather than a panic or a stack overflow.
#[test]
fn deeply_nested_source_is_refused_rather_than_overflowing_the_stack() {
    let (step, context, receipt, program) = fixture();
    let deep = format!("rule r() = {}1{}\n", "(".repeat(5_000), ")".repeat(5_000));

    match check_l3_audit_receipt_from_source_v1(
        deep.as_bytes(),
        program,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &receipt,
        &step,
        context,
    ) {
        Err(SourceReceiptError::Parse(msg)) => {
            assert!(msg.contains("resource limit"), "got {msg}");
        }
        other => panic!("deep nesting must be refused as a resource limit, got {other:?}"),
    }
}

/// **D6.** An oversized source is refused before it is even decoded.
///
/// This previously asserted `SourceReceiptError::Parse`, which is produced
/// *after* `from_utf8` has already validated the whole slice — so the test
/// could not show the property its name and doc claimed. It now asserts the
/// refusal that fires first; `oversized_source_is_refused_before_utf8_validation`
/// below pins the ordering itself.
#[test]
fn oversized_source_is_refused_before_tokenization() {
    let (step, context, receipt, program) = fixture();
    let limits = ParseLimits {
        max_source_bytes: 16,
        ..ParseLimits::strict()
    };

    match check_l3_audit_receipt_from_source_v1(
        SRC.as_bytes(),
        program,
        limits,
        &PlanLimitsV1::generous(),
        &receipt,
        &step,
        context,
    ) {
        Err(SourceReceiptError::SourceTooLarge { limit, found }) => {
            assert_eq!(limit, 16);
            assert_eq!(found, SRC.len());
        }
        other => panic!("oversized source must be refused, got {other:?}"),
    }
}

/// **D6, the ordering itself.** `ParseLimits::max_source_bytes` documents
/// itself as "checked before UTF-8 conversion or tokenization". Asserting a
/// refusal on oversized input does not show that — a bound applied *after*
/// `from_utf8` refuses the same input, having already done the O(n) work the
/// bound exists to prevent.
///
/// So this feeds input that is **both** oversized and invalid UTF-8. The two
/// checks disagree about which error to produce, and only the one that runs
/// first can produce its own. `SourceTooLarge` is therefore the discriminating
/// observation, and `InvalidUtf8` here would mean the ordering had regressed.
#[test]
fn oversized_source_is_refused_before_utf8_validation() {
    let (step, context, receipt, program) = fixture();
    let limits = ParseLimits {
        max_source_bytes: 16,
        ..ParseLimits::strict()
    };
    // Invalid UTF-8, and longer than the 16-byte bound.
    let hostile = vec![0xffu8; 64];

    match check_l3_audit_receipt_from_source_v1(
        &hostile,
        program,
        limits,
        &PlanLimitsV1::generous(),
        &receipt,
        &step,
        context,
    ) {
        Err(SourceReceiptError::SourceTooLarge { limit, found }) => {
            assert_eq!(limit, 16);
            assert_eq!(found, 64);
        }
        Err(SourceReceiptError::InvalidUtf8) => {
            panic!("the size bound must fire before UTF-8 validation, not after it")
        }
        other => panic!("oversized source must be refused, got {other:?}"),
    }
}

/// **D8.** Non-UTF-8 bytes are refused, not lossily decoded.
#[test]
fn invalid_utf8_is_refused() {
    let (step, context, receipt, program) = fixture();
    match check_l3_audit_receipt_from_source_v1(
        &[0xff, 0xfe, 0xfd],
        program,
        ParseLimits::strict(),
        &PlanLimitsV1::generous(),
        &receipt,
        &step,
        context,
    ) {
        Err(SourceReceiptError::InvalidUtf8) => {}
        other => panic!("invalid UTF-8 must be refused, got {other:?}"),
    }
}

/// **D3/D9 — the reason this beats the signing path.** The derived manifest is
/// the one the receipt is checked against, and it is *derived*, not supplied.
/// A fabricated manifest, however internally consistent, has a different id
/// than the one source re-derivation produces.
#[test]
fn the_locally_derived_manifest_is_what_a_fabricated_one_disagrees_with() {
    let module = parse(SRC).expect("parses");
    let plan =
        lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous()).expect("lowers");
    let mut interner = Interner::new();
    let table = build_l3_transition_table(&mut interner, &plan);
    let derived = l3_generator_semantics(&table);

    // A fabricated declaration over the same generators.
    let mut fabricated = brix_semantic::GeneratorSemanticsV1::new();
    for g in l3_generator_registry(&table).iter() {
        fabricated.declare_diagonal(*g);
    }

    assert_ne!(
        derived.id(),
        fabricated.id(),
        "a fabricated manifest must not share the derived manifest's id"
    );
}
