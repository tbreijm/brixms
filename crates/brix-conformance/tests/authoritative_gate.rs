//! Authoritative-flip SPIKE (#15): proof that `brix.type`'s own derived
//! type-conflict verdict can *gate* — reject a program — rather than only be
//! compared against `reflect.rs` in the shadow-mode parity harness.
//!
//! Everything the native package does today runs in SHADOW mode
//! (`selfhost_parity.rs`): its derived `*Conflict` extents are decoded and
//! asserted byte-equal to `reflect.rs`'s own conflicts, but the package has
//! zero `constraint`s and can never reject a transaction
//! (`no_constraint_based_rejection_in_the_package_source`). This spike takes
//! the smallest real step toward *authoritative* mode: it compiles a
//! **constrained variant** of the package — the exact same rule set plus one
//! `constraint MismatchGate strict { MismatchConflict(...) }` — through the
//! real `brix.type` pipeline, feeds it a program `reflect::analyze` reports as
//! type-inconsistent, and confirms the native `MismatchConflict` the package
//! derives makes `settle().strict_ok()` go **false**: the verdict rejects.
//!
//! Scope (deliberate): this proves the *gating mechanism* composes with the
//! exporter's opaque-token bridge (which until now was built and validated
//! only for *comparison*, never *enforcement*). It does NOT make the package
//! independent of `reflect.rs` — the inputs still come from
//! `typefacts::export(&reflect::analyze(...))`. Retiring `reflect.rs` is a
//! separate, much larger effort (running unification natively); this spike
//! only de-risks that the authoritative *plumbing* is real.
//!
//! The real `packages/brix.type/brix.type.brix` is left constraint-free
//! (shadow mode's invariant); the constraint is appended to an in-memory copy
//! here, so this is a distinct *run mode*, not a change to the shared source.

use brix_ast::parse_file;
use brix_ir::reflect::{analyze, ConflictKind, ReflectiveReport, Subject};
use brix_rt::engine::{apply_transaction, settle, GroundState, Program, Transaction};
use brixc::pipeline::PhaseAssign;
use brixc::{emit, lower_file, AstPhase};

use brix_conformance::typecorpus::{
    NATIVE_ROLE_BINDINGS_FIXTURE, NATIVE_ROLE_LIT_MISMATCH_FIXTURE,
};
use brixc::selfhost::typefacts;

const PACKAGE_SRC: &str = include_str!("../../../packages/brix.type/brix.type.brix");

/// The one thing this spike adds to the shadow-mode package: a strict
/// constraint that fires whenever the native rules derive ANY
/// `MismatchConflict` row. A `constraint` body is a query — a bare relation
/// match is satisfied exactly when the relation is non-empty — so this rejects
/// precisely the programs `brix.type` itself judges type-inconsistent.
const MISMATCH_GATE: &str = "
constraint MismatchGate strict {
    MismatchConflict(subject: s, expect: e, found: f, scope: sc)
}
";

fn analyze_source(src: &str) -> ReflectiveReport {
    let (file, parse_diags) = parse_file(src);
    assert!(
        !parse_diags.has_errors(),
        "fixture must parse cleanly: {:#?}",
        parse_diags.iter().collect::<Vec<_>>()
    );
    let lowered = lower_file(&file, &parse_diags);
    analyze(&lowered.source, &lowered.resolver)
}

/// Compile `brix.type` + the appended `MismatchGate` constraint through the
/// real native pipeline (`lower_file` → `assign_phases` → `project_program`),
/// exactly as `selfhost_parity::compiled_package` does for the unconstrained
/// package. This also exercises that the constraint STRATIFIES cleanly over
/// the derived `MismatchConflict` relation (Appendix F) — a real unknown,
/// since no `constraint` has ever sat alongside these `derive`s before.
fn compiled_constrained_package() -> Program {
    let src = format!("{PACKAGE_SRC}{MISMATCH_GATE}");
    let (file, parse_diags) = parse_file(&src);
    assert!(
        !parse_diags.has_errors(),
        "constrained brix.type must parse cleanly: {:#?}",
        parse_diags.iter().collect::<Vec<_>>()
    );
    let lowered = lower_file(&file, &parse_diags);
    assert!(
        !lowered.has_errors(),
        "constrained brix.type must lower and type-check cleanly: {:#?}",
        lowered.diags
    );
    assert_eq!(
        lowered.source.constraints.len(),
        1,
        "the appended MismatchGate constraint must lower to exactly one constraint"
    );
    let phased = AstPhase.assign_phases(lowered).expect(
        "constrained brix.type must be well-stratified (Appendix F) — the strict \
         constraint reads the derived MismatchConflict relation, so it must phase-assign \
         strictly after every MismatchConflict producer",
    );
    emit::project_program(&phased)
}

/// THE FLIP, positive direction: a program `reflect` reports as
/// type-inconsistent (a literal bound to a role whose declared type disagrees)
/// must be REJECTED by `brix.type`'s own strict `MismatchGate` — the native
/// verdict is authoritative, not merely observed.
#[test]
fn native_mismatch_verdict_gates_a_strict_rejection() {
    let report = analyze_source(NATIVE_ROLE_LIT_MISMATCH_FIXTURE);
    // Sanity: reflect itself sees exactly one Mismatch here (the twin fixture).
    assert_eq!(
        report
            .conflicts
            .iter()
            .filter(|c| matches!(c.kind, ConflictKind::Mismatch { .. }))
            .count(),
        1,
        "reflect must report exactly one Mismatch for the role-lit-mismatch fixture"
    );

    let program = compiled_constrained_package();
    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"authoritative-gate-mismatch".to_vec());
    txn.ops = export.ops;

    // Drive settle() directly (not Store::commit) so we get the verdict AND
    // the decodable "why" in one shot — commit() discards the candidate
    // Settled on the strict-error path and its error carries only a revision
    // number, no diagnostic.
    let ground = apply_transaction(&program, &GroundState::default(), &txn)
        .expect("exported facts must assert cleanly into ground (shadow-mode inputs are valid)");
    let settled = settle(&program, &ground, 1);

    assert!(
        !settled.strict_ok(&program),
        "brix.type's own derived MismatchConflict must trip the strict MismatchGate — \
         a type-inconsistent program is REJECTED by the native verdict, end to end"
    );
    assert!(
        !settled.violations.is_empty(),
        "the strict rejection must record at least one Violation"
    );

    // The rejection reason is recoverable from the settled extent through the
    // same opaque-token bridge the parity harness uses — proving an
    // authoritative checker can emit a real diagnostic, not just a yes/no.
    let extent = settled
        .extents
        .get("MismatchConflict")
        .expect("the constrained package must still expose the MismatchConflict extent");
    assert_eq!(extent.len(), 1, "exactly one gating MismatchConflict row");
    let record = extent.values().next().unwrap();
    let resolved = typefacts::resolve_mismatch(&export.tokens, &record.row).expect(
        "the gating conflict's opaque tokens must decode back to a real (subject, expect, found) \
         — enforcement composes with the comparison-oriented token bridge",
    );
    assert!(
        matches!(resolved.subject, Subject::Binding { .. }),
        "the gating mismatch must be attributed to the offending role binding, got {:?}",
        resolved.subject
    );
}

/// THE FLIP, negative direction: a well-typed program must NOT be rejected —
/// the gate discriminates on the native verdict; it does not reject
/// everything. Without this, "it rejected" would be meaningless.
#[test]
fn native_verdict_admits_a_well_typed_program() {
    let report = analyze_source(NATIVE_ROLE_BINDINGS_FIXTURE);
    assert!(
        report
            .conflicts
            .iter()
            .all(|c| !matches!(c.kind, ConflictKind::Mismatch { .. })),
        "the smallest role-bindings fixture must be Mismatch-free in reflect"
    );

    let program = compiled_constrained_package();
    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"authoritative-gate-clean".to_vec());
    txn.ops = export.ops;

    let ground = apply_transaction(&program, &GroundState::default(), &txn)
        .expect("exported facts must assert cleanly into ground");
    let settled = settle(&program, &ground, 1);

    assert!(
        settled.strict_ok(&program),
        "a well-typed program must pass the strict MismatchGate — the native verdict admits it"
    );
    assert!(
        settled.violations.is_empty(),
        "a well-typed program must record zero violations"
    );
}
