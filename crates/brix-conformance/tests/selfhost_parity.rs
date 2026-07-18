//! Shadow-mode differential parity harness (#15 native `brix.type` vertical
//! slice 1).
//!
//! For each fixture below: `brix_ir::reflect::analyze` produces the
//! reference report; `brix_conformance::typefacts::export` flattens its
//! `RoleVar`/`RoleLit`/`SchemaRole` facts (plus the `RootScope` singleton)
//! into opaque canonical tokens and Ground `Assert` ops; those ops are
//! committed, in-process, against `packages/brix.type/brix.type.brix`
//! compiled through the real `.brix` -> `brix_rt::engine::Program` path
//! (`brixc::lower_file` -> `AstPhase::assign_phases` ->
//! `emit::project_program`); the settled derived extents are mapped back
//! through the token table to `Subject`/`Ty`/`ScopeId` and compared against
//! `reflect.rs`'s own report by literal `FactId` (or canonical conflict
//! byte) equality.
//!
//! **Shadow mode only** (#15 acceptance): this test is the parity oracle,
//! not a gate — nothing outside this file depends on the native package's
//! verdicts, and the package has no `constraint`, so it can never reject a
//! transaction.

use std::collections::BTreeSet;

use brix_ast::parse_file;
use brix_canon::CanonWriter;
use brix_ir::reflect::{
    analyze, write_conflict, ConflictKind, Fact, FactId, Subject, TypeConflict,
};
use brix_rt::engine::{Program, Store, Transaction};
use brixc::pipeline::PhaseAssign;
use brixc::{emit, lower_file, AstPhase};

use brix_conformance::typefacts;

/// The smallest fixture (#15 slice-1 ruling): a two-role body clause with
/// both roles bound to variables. `reflect.rs` records exactly two
/// `Fact::HasType(Subject::Binding)` facts for it (`count`, `label`) — the
/// native package, driven only by the `RoleVar` facts `role_arg` now emits,
/// must derive `FactId`-for-`FactId` the same two.
const FIXTURE_ROLE_BINDINGS: &str = r#"
package t @ 1.0.0

rel Input { count: Int; label: String } key(count)
rel Output { count: Int } key(count)

derive Copy: Output(count: count) from {
    Input(count, label)
}
"#;

/// The twin fixture: `label`'s role argument is a literal of the wrong
/// class (`Int` where the schema declares `String`) instead of a variable.
/// `count` stays a plain variable so the rule head (`Output`, keyed on
/// `count`) still binds cleanly — the #15 ruling explicitly allows mismatch
/// on `count` *or* `label`; `label` is chosen here so the twin doesn't also
/// need to work around an unrelated unbound-head-key Appendix-E finding.
/// `reflect.rs` raises exactly one `ConflictKind::Mismatch` for this, at
/// `Subject::Binding { declaration: "Copy", name: "label" }` (`role_arg`'s
/// existing behavior, unchanged by this slice) — the native package's
/// `LitRoleMismatch` rule, driven by the new `RoleLit` fact, must derive
/// exactly one oriented `MismatchConflict` row that decodes back to the
/// identical (subject, expect, found, scope).
const FIXTURE_ROLE_LIT_MISMATCH: &str = r#"
package t @ 1.0.0

rel Input { count: Int; label: String } key(count)
rel Output { count: Int } key(count)

derive Copy: Output(count: count) from {
    Input(count: count, label: 5)
}
"#;

const PACKAGE_SRC: &str = include_str!("../../../packages/brix.type/brix.type.brix");

fn analyze_source(src: &str) -> brix_ir::reflect::ReflectiveReport {
    let (file, parse_diags) = parse_file(src);
    assert!(
        !parse_diags.has_errors(),
        "fixture must parse cleanly: {:#?}",
        parse_diags.iter().collect::<Vec<_>>()
    );
    let lowered = lower_file(&file, &parse_diags);
    analyze(&lowered.source, &lowered.resolver)
}

/// Compile `packages/brix.type/brix.type.brix` through the real native
/// pipeline (the anchor this slice was gated on: #10/#11/PR3.5, all on
/// `main`) — never a hand-built `Program`.
fn compiled_package() -> Program {
    let (file, parse_diags) = parse_file(PACKAGE_SRC);
    assert!(
        !parse_diags.has_errors(),
        "brix.type package must parse cleanly: {:#?}",
        parse_diags.iter().collect::<Vec<_>>()
    );
    let lowered = lower_file(&file, &parse_diags);
    assert!(
        !lowered.has_errors(),
        "brix.type package must lower and type-check cleanly: {:#?}",
        lowered.diags
    );
    let phased = AstPhase
        .assign_phases(lowered)
        .expect("brix.type package must be well-stratified (Appendix F)");
    emit::project_program(&phased)
}

#[test]
fn no_constraint_based_rejection_in_the_package_source() {
    // Shadow mode must never reject a transaction (#15 acceptance bar): the
    // package source must not declare a `constraint` anywhere. A cheap,
    // direct textual guard alongside the structural one below.
    assert!(
        !PACKAGE_SRC
            .lines()
            .any(|line| line.trim_start().starts_with("constraint ")),
        "packages/brix.type/brix.type.brix must not declare a `constraint` \
         (shadow mode must never reject a transaction)"
    );
    let (file, parse_diags) = parse_file(PACKAGE_SRC);
    let lowered = lower_file(&file, &parse_diags);
    assert!(
        lowered.source.constraints.is_empty(),
        "brix.type package must not lower any `constraint` declaration"
    );
}

#[test]
fn smallest_fixture_two_role_bindings_agree_fact_id_for_fact_id() {
    let report = analyze_source(FIXTURE_ROLE_BINDINGS);

    // Sanity on the reference side first: exactly two `HasType(Binding)`
    // facts, matching the #15 slice-1 "smallest first fixture" spec.
    let expected: BTreeSet<FactId> = report
        .facts
        .iter()
        .filter(|derivation| {
            matches!(
                &derivation.fact,
                Fact::HasType {
                    subject: Subject::Binding { .. },
                    ..
                }
            )
        })
        .map(|derivation| derivation.id)
        .collect();
    assert_eq!(
        expected.len(),
        2,
        "reflect.rs must record exactly two HasType(Binding) facts for the \
         smallest fixture; got {expected:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-role-bindings".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let has_type_extent = settled
        .extents
        .get("HasType")
        .expect("brix.type package must declare a HasType relation");
    assert_eq!(
        has_type_extent.len(),
        2,
        "native package must derive exactly two HasType(Binding) facts"
    );

    let native: BTreeSet<FactId> = has_type_extent
        .values()
        .map(|record| {
            let fact = typefacts::resolve_has_type(&export.tokens, &record.row)
                .expect("every derived HasType row's tokens must resolve through the token table");
            FactId::derive(&fact)
        })
        .collect();

    assert_eq!(
        native, expected,
        "native-derived HasType FactIds must equal reflect.rs's, FactId-for-FactId"
    );
}

#[test]
fn twin_fixture_derives_exactly_one_oriented_mismatch_conflict() {
    let report = analyze_source(FIXTURE_ROLE_LIT_MISMATCH);

    let reflect_conflicts: Vec<&TypeConflict> = report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::Mismatch { .. }))
        .collect();
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one Mismatch conflict for the twin fixture; got {reflect_conflicts:?}"
    );
    let reflect_conflict = reflect_conflicts[0];
    assert_eq!(
        reflect_conflict.subject,
        Subject::Binding {
            declaration: brix_ir::ident::Ident::new("Copy"),
            name: brix_ir::ident::Ident::new("label"),
        },
        "the reference mismatch must be attributed to the label role binding"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-role-lit-mismatch".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let mismatch_extent = settled
        .extents
        .get("MismatchConflict")
        .expect("brix.type package must declare a MismatchConflict relation");
    assert_eq!(
        mismatch_extent.len(),
        1,
        "native package must derive exactly one oriented MismatchConflict row"
    );

    let record = mismatch_extent.values().next().unwrap();
    let resolved = typefacts::resolve_mismatch(&export.tokens, &record.row)
        .expect("the derived MismatchConflict row's tokens must resolve through the token table");

    let native_conflict = TypeConflict {
        subject: resolved.subject,
        kind: ConflictKind::Mismatch {
            left: resolved.expect,
            right: resolved.found,
        },
        because: BTreeSet::new(),
        scope: resolved.scope,
    };

    let mut native_bytes = CanonWriter::new();
    write_conflict(&native_conflict, &mut native_bytes);
    let mut reflect_bytes = CanonWriter::new();
    write_conflict(reflect_conflict, &mut reflect_bytes);

    assert_eq!(
        native_bytes.finish(),
        reflect_bytes.finish(),
        "the native-derived MismatchConflict must be canonical-byte-identical \
         to reflect.rs's own conflict (same subject, same orientation, same scope)"
    );
}
