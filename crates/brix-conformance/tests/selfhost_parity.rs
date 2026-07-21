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
use brix_ir::ident::Ident;
use brix_ir::reflect::{
    analyze, write_conflict, ConflictKind, Fact, FactId, ReflectiveReport, Subject, TypeConflict,
};
use brix_ir::types::{IntWidth, Ty};
use brix_rt::engine::{Extent, Program, Store, Transaction};
use brixc::pipeline::PhaseAssign;
use brixc::{emit, lower_file, AstPhase};

use brix_conformance::typecorpus::{
    field_failure, occurs_check, occurs_check_row, NATIVE_GUARD_NON_BOOL_FIXTURE,
    NATIVE_OPERATOR_APPLIES_FIXTURE, NATIVE_ROLE_BINDINGS_FIXTURE,
    NATIVE_ROLE_LIT_MISMATCH_FIXTURE, NATIVE_VAR_SAME_ROLE_TWICE_FIXTURE,
    NATIVE_VAR_THREE_ROLES_FIXTURE, NATIVE_VAR_TWO_ROLES_MISMATCH_FIXTURE,
    NATIVE_WHEN_REQUIRES_BOOL_FIXTURE,
};
use brix_conformance::typefacts;

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

/// Canonical `write_conflict` bytes for one [`TypeConflict`] — the atom the
/// #15 slice-2 ruling's set-comparison rule operates over (extends PR2's
/// "categories compared as canonical sets, never sequences" discipline to
/// the conflict-byte comparison this harness makes).
fn conflict_bytes(conflict: &TypeConflict) -> Vec<u8> {
    let mut w = CanonWriter::new();
    write_conflict(conflict, &mut w);
    w.finish()
}

/// A **set** of canonical conflict bytes, never a sequence — two conflicts
/// with identical `(subject, kind, scope)` collapse under canonical bytes,
/// matching the native `MismatchConflict` relation's own set semantics by
/// key (#15 slice-2 ruling §1).
fn conflict_byte_set<'a>(
    conflicts: impl IntoIterator<Item = &'a TypeConflict>,
) -> BTreeSet<Vec<u8>> {
    conflicts.into_iter().map(conflict_bytes).collect()
}

/// Every `Mismatch`-kind conflict `reflect::analyze` recorded.
fn reflect_mismatches(report: &ReflectiveReport) -> Vec<&TypeConflict> {
    report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::Mismatch { .. }))
        .collect()
}

/// Resolve every row of a settled `MismatchConflict` extent back to a
/// comparable [`TypeConflict`] via the exporter's token table — the native
/// counterpart of [`reflect_mismatches`].
fn native_mismatches(tokens: &typefacts::TokenTable, extent: &Extent) -> Vec<TypeConflict> {
    extent
        .values()
        .map(|record| {
            let resolved = typefacts::resolve_mismatch(tokens, &record.row).expect(
                "every derived MismatchConflict row's tokens must resolve through the token table",
            );
            TypeConflict {
                subject: resolved.subject,
                kind: ConflictKind::Mismatch {
                    left: resolved.expect,
                    right: resolved.found,
                },
                because: BTreeSet::new(),
                scope: resolved.scope,
            }
        })
        .collect()
}

/// Every `Fact::RoleVar` `reflect::analyze` recorded, subject+ordinal only
/// (enough to assert traversal-order occurrence indices without depending on
/// the rest of the fact's shape).
fn role_var_ordinals(report: &ReflectiveReport) -> Vec<u32> {
    let mut ordinals: Vec<u32> = report
        .facts
        .iter()
        .filter_map(|derivation| match &derivation.fact {
            Fact::RoleVar { ordinal, .. } => Some(*ordinal),
            _ => None,
        })
        .collect();
    ordinals.sort_unstable();
    ordinals
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
    let report = analyze_source(NATIVE_ROLE_BINDINGS_FIXTURE);

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
    let report = analyze_source(NATIVE_ROLE_LIT_MISMATCH_FIXTURE);

    let reflect_conflicts = reflect_mismatches(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one Mismatch conflict for the twin fixture; got {reflect_conflicts:?}"
    );
    assert_eq!(
        reflect_conflicts[0].subject,
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

    let native_conflicts = native_mismatches(&export.tokens, mismatch_extent);

    // #15 slice-2 ruling: conflicts compare as canonical-byte **sets**, never
    // one-vs-one — this generalizes cleanly to the singleton case here.
    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived MismatchConflict set must be canonical-byte-identical \
         to reflect.rs's own conflict set (same subject, same orientation, same scope)"
    );
}

/// #15 slice-2 ruling (Fable comment 5012408628, §5 "Confused"): a var
/// role-bound at two roles with disagreeing declared types (`Int` then
/// `String`) must derive exactly one `HasType` (typed by the FIRST
/// occurrence) and exactly one oriented `Mismatch` — never two contradictory
/// root-world `HasType`s and never a clean bill of health, which is what the
/// pre-slice-2 native package derived for this exact program.
#[test]
fn var_two_roles_mismatch_yields_one_has_type_and_one_oriented_mismatch() {
    let report = analyze_source(NATIVE_VAR_TWO_ROLES_MISMATCH_FIXTURE);

    // Reference side, exact expectations from the ruling.
    assert_eq!(
        role_var_ordinals(&report),
        vec![0, 1],
        "reflect.rs must record two RoleVar facts for `count`, ordinals 0 and 1"
    );

    let has_type_bindings: Vec<&Fact> = report
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
        .map(|derivation| &derivation.fact)
        .collect();
    assert_eq!(
        has_type_bindings,
        vec![&Fact::HasType {
            subject: Subject::Binding {
                declaration: Ident::new("Confused"),
                name: Ident::new("count"),
            },
            ty: Ty::Int(IntWidth::Int),
            scope: brix_ir::reflect::ScopeId::root(),
        }],
        "reflect.rs must record exactly one HasType(Binding) fact, typed by \
         the FIRST occurrence (Int), never a second contradictory HasType"
    );

    let reflect_conflicts = reflect_mismatches(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one Mismatch conflict; got {reflect_conflicts:?}"
    );
    assert_eq!(
        reflect_conflicts[0].kind,
        ConflictKind::Mismatch {
            left: Ty::Int(IntWidth::Int),
            right: Ty::Str,
        },
        "the conflict must be oriented against the first occurrence (Int), not later-vs-later"
    );
    assert!(
        !report.is_consistent(),
        "a var-at-two-roles disagreement must make the reference report inconsistent"
    );

    // Native side.
    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-var-two-roles-mismatch".to_vec());
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
        1,
        "native package must derive exactly one HasType row, not two contradictory ones"
    );
    let native_has_type_id = {
        let record = has_type_extent.values().next().unwrap();
        let fact = typefacts::resolve_has_type(&export.tokens, &record.row)
            .expect("the derived HasType row's tokens must resolve through the token table");
        FactId::derive(&fact)
    };
    let reflect_has_type_id = report
        .facts
        .iter()
        .find(|derivation| {
            matches!(
                &derivation.fact,
                Fact::HasType {
                    subject: Subject::Binding { .. },
                    ..
                }
            )
        })
        .expect("reflect.rs must have recorded the Binding HasType above")
        .id;
    assert_eq!(
        native_has_type_id, reflect_has_type_id,
        "the native-derived HasType must be FactId-equal to reflect.rs's own"
    );

    let mismatch_extent = settled
        .extents
        .get("MismatchConflict")
        .expect("brix.type package must declare a MismatchConflict relation");
    assert_eq!(
        mismatch_extent.len(),
        1,
        "native package must derive exactly one MismatchConflict row"
    );
    let native_conflicts = native_mismatches(&export.tokens, mismatch_extent);
    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived MismatchConflict must canonical-byte-equal reflect.rs's own"
    );
}

/// #15 slice-2 ruling follow-up corpus: a var role-bound at the *same* role
/// twice. Before slice-2, the two `RoleVar` facts were byte-identical (no
/// `ordinal`) and collapsed to one `FactId`, silently under-reporting the
/// program. This proves the duplicate no longer collapses, that agreeing
/// occurrences still yield exactly one `HasType` and zero conflicts, and —
/// the sharper native regression the ordinal-keyed `RoleVar` key exists to
/// prevent — that the export still commits cleanly (no `GroundKeyConflict`).
#[test]
fn var_same_role_twice_yields_two_role_var_rows_one_has_type_zero_conflicts() {
    let report = analyze_source(NATIVE_VAR_SAME_ROLE_TWICE_FIXTURE);

    assert_eq!(
        role_var_ordinals(&report),
        vec![0, 1],
        "reflect.rs must record two distinct RoleVar facts (ordinals 0, 1) \
         for a var bound at the same role twice, not one collapsed fact"
    );

    let has_type_bindings = report
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
        .count();
    assert_eq!(
        has_type_bindings, 1,
        "agreeing same-role-twice occurrences must still yield exactly one HasType"
    );
    assert!(
        reflect_mismatches(&report).is_empty(),
        "agreeing occurrences must not raise a Mismatch conflict"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-var-same-role-twice".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store.commit(&txn).expect(
        "exported facts must commit cleanly — the ordinal-keyed RoleVar key must not \
         raise a GroundKeyConflict on a var bound at the same role twice",
    );

    let role_var_extent = settled
        .extents
        .get("RoleVar")
        .expect("brix.type package must declare a RoleVar relation");
    assert_eq!(
        role_var_extent.len(),
        2,
        "the two per-occurrence RoleVar rows must both be present, not collapsed"
    );

    let has_type_extent = settled
        .extents
        .get("HasType")
        .expect("brix.type package must declare a HasType relation");
    assert_eq!(
        has_type_extent.len(),
        1,
        "native package must derive exactly one HasType row"
    );

    let mismatch_extent = settled
        .extents
        .get("MismatchConflict")
        .expect("brix.type package must declare a MismatchConflict relation");
    assert_eq!(
        mismatch_extent.len(),
        0,
        "native package must derive zero MismatchConflict rows for agreeing occurrences"
    );
}

/// #15 slice-2 ruling follow-up corpus (§4 multiplicity check): a var
/// role-bound at three roles, declared `Int, String, Bool` in traversal
/// order. Exactly one `HasType` (Int) and exactly two oriented conflicts —
/// `(Int, Str)` and `(Int, Bool)` — and, critically, never `(Str, Bool)`:
/// later-vs-later pairs are structurally never derived, by either checker.
#[test]
fn var_three_roles_yields_two_oriented_mismatches_never_later_vs_later() {
    let report = analyze_source(NATIVE_VAR_THREE_ROLES_FIXTURE);

    assert_eq!(
        role_var_ordinals(&report),
        vec![0, 1, 2],
        "reflect.rs must record three RoleVar facts, ordinals 0, 1, 2"
    );

    let reflect_conflicts = reflect_mismatches(&report);
    let reflect_kinds: BTreeSet<(Ty, Ty)> = reflect_conflicts
        .iter()
        .map(|conflict| match &conflict.kind {
            ConflictKind::Mismatch { left, right } => (left.clone(), right.clone()),
            other => panic!("expected a Mismatch conflict, got {other:?}"),
        })
        .collect();
    assert_eq!(
        reflect_kinds,
        BTreeSet::from([
            (Ty::Int(IntWidth::Int), Ty::Str),
            (Ty::Int(IntWidth::Int), Ty::Bool),
        ]),
        "reflect.rs must derive exactly the two oriented (first, later) pairs, \
         never the later-vs-later (Str, Bool) pair"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-var-three-roles".to_vec());
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
        1,
        "native package must derive exactly one HasType row"
    );

    let mismatch_extent = settled
        .extents
        .get("MismatchConflict")
        .expect("brix.type package must declare a MismatchConflict relation");
    assert_eq!(
        mismatch_extent.len(),
        2,
        "native package must derive exactly two oriented MismatchConflict rows"
    );
    let native_conflicts = native_mismatches(&export.tokens, mismatch_extent);

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived MismatchConflict set must canonical-byte-equal reflect.rs's own, \
         and by this equality never contain the (Str, Bool) later-vs-later pair"
    );
}

/// #15 native slice 3 (RequiresBool): a single `when` clause with a
/// well-typed `Bool` condition. `reflect.rs`'s `Clause::When` handling
/// records `Fact::RequiresBool { subject: Subject::Expr{origin}, scope:
/// ScopeId::root() }` unconditionally for every `when` clause, before it
/// even checks whether the condition's type is `Bool` — a direct
/// restatement, not a join. The native package reproduces this via the
/// `WhenCond` structural input the exporter now emits for
/// `Fact::RequiresBool`, re-derived through a `RootScope` join
/// (`RequiresBoolInRoot`).
#[test]
fn when_clause_derives_requires_bool_fact_id_for_fact_id() {
    let report = analyze_source(NATIVE_WHEN_REQUIRES_BOOL_FIXTURE);

    let expected: BTreeSet<FactId> = report
        .facts
        .iter()
        .filter(|derivation| matches!(&derivation.fact, Fact::RequiresBool { .. }))
        .map(|derivation| derivation.id)
        .collect();
    assert_eq!(
        expected.len(),
        1,
        "reflect.rs must record exactly one RequiresBool fact for the single-when-clause \
         fixture; got {expected:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-when-requires-bool".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let requires_bool_extent = settled
        .extents
        .get("RequiresBool")
        .expect("brix.type package must declare a RequiresBool relation");
    assert_eq!(
        requires_bool_extent.len(),
        1,
        "native package must derive exactly one RequiresBool row"
    );

    let native: BTreeSet<FactId> = requires_bool_extent
        .values()
        .map(|record| {
            let fact = typefacts::resolve_requires_bool(&export.tokens, &record.row).expect(
                "every derived RequiresBool row's tokens must resolve through the token table",
            );
            FactId::derive(&fact)
        })
        .collect();

    assert_eq!(
        native, expected,
        "native-derived RequiresBool FactIds must equal reflect.rs's, FactId-for-FactId"
    );
}

/// #15 native slice 4 (Applies): a single `x + 1` operator application.
/// `reflect.rs`'s `Reflect::call` records `Fact::Applies { subject:
/// Subject::Expr{origin}, operator: func.to_string(), scope: root }` for every
/// call/operator node — a direct restatement, not a join. The native package
/// reproduces it via the `OpApply` structural input the exporter now emits for
/// `Fact::Applies`, re-derived through a `RootScope` join (`AppliesInRoot`).
/// The `operator` string round-trips verbatim (it was never a token), so the
/// derived `FactId` must match reflect's own.
#[test]
fn operator_application_derives_applies_fact_id_for_fact_id() {
    let report = analyze_source(NATIVE_OPERATOR_APPLIES_FIXTURE);

    let expected: BTreeSet<FactId> = report
        .facts
        .iter()
        .filter(|derivation| matches!(&derivation.fact, Fact::Applies { .. }))
        .map(|derivation| derivation.id)
        .collect();
    assert_eq!(
        expected.len(),
        1,
        "reflect.rs must record exactly one Applies fact for the single-operator \
         fixture; got {expected:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-operator-applies".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let applies_extent = settled
        .extents
        .get("Applies")
        .expect("brix.type package must declare an Applies relation");
    assert_eq!(
        applies_extent.len(),
        expected.len(),
        "native package must derive exactly as many Applies rows as reflect.rs"
    );

    let native: BTreeSet<FactId> = applies_extent
        .values()
        .map(|record| {
            let fact = typefacts::resolve_applies(&export.tokens, &record.row)
                .expect("every derived Applies row's tokens must resolve through the token table");
            FactId::derive(&fact)
        })
        .collect();

    assert_eq!(
        native, expected,
        "native-derived Applies FactIds must equal reflect.rs's, FactId-for-FactId \
         (including the verbatim operator string)"
    );
}

/// #15 native slice 5 (NonBool): a `when n` guard whose bound variable `n` is
/// `Int`. `reflect.rs` records `HasType{Subject::Expr(n), Int}` for the guard
/// and, since `Int != Bool && !is_var(Int)`, a `ConflictKind::NonBool{found:
/// Int}`. The native package reproduces the conflict from the imported
/// `ExprType` (reflect's post-inference expr type, with `Ty::Var` rows dropped
/// on the bridge to honor reflect's `!is_var` guard) joined against the
/// `BoolType` singleton by `GuardNonBool` — the first slice to consume
/// reflect's inferred types rather than re-derive them.
#[test]
fn non_bool_guard_derives_one_non_bool_conflict() {
    let report = analyze_source(NATIVE_GUARD_NON_BOOL_FIXTURE);

    let reflect_conflicts: Vec<&TypeConflict> = report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::NonBool { .. }))
        .collect();
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one NonBool conflict for the `when n` \
         (n: Int) fixture; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-non-bool".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let non_bool_extent = settled
        .extents
        .get("NonBoolConflict")
        .expect("brix.type package must declare a NonBoolConflict relation");
    assert_eq!(
        non_bool_extent.len(),
        1,
        "native package must derive exactly one NonBoolConflict row"
    );

    let native_conflicts: Vec<TypeConflict> = non_bool_extent
        .values()
        .map(|record| {
            let resolved = typefacts::resolve_non_bool(&export.tokens, &record.row).expect(
                "every derived NonBoolConflict row's tokens must resolve through the token table",
            );
            TypeConflict {
                subject: resolved.subject,
                kind: ConflictKind::NonBool {
                    found: resolved.found,
                },
                because: BTreeSet::new(),
                scope: resolved.scope,
            }
        })
        .collect();

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived NonBool conflict set must canonical-byte-equal reflect.rs's own \
         (same subject, same `found` type, same scope)"
    );
}

/// #15 native slice 6 (UnknownField, field-access form): the package's FIRST
/// negation. Uses the builder-based `field_failure` fixture — a `record.absent`
/// access where the base row is `{present: Int}` — because brixc lowers
/// `base.field` in a rule/derive/constraint body as a qualified `Path`, never
/// an `ExprKind::Field` (that only surfaces in driver/match contexts reflect
/// doesn't analyze), so the field-access form isn't expressible as `.brix`
/// source through this harness. Building the fixture's report directly (as the
/// 19 `type_parity` fixtures already do) still exercises the *real* native
/// package. `reflect.rs`'s `ExprKind::Field` arm raises `UnknownField{absent}`
/// (the base's row has `present`, not `absent`); the native `FieldNotInRow`
/// rule derives the same conflict from the *absence* of `RowField(base, absent)`
/// under `without` — proving stratified negation compiles, phase-assigns, and
/// settles in the real engine, byte-for-byte with reflect. The base's genuine
/// `RowField(base, present)` (which the `without` must NOT match) makes this a
/// real discrimination, not a vacuous negation.
#[test]
fn unknown_field_access_derives_one_unknown_field_conflict() {
    let fixture = field_failure();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts: Vec<&TypeConflict> = report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::UnknownField { .. }))
        .collect();
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one UnknownField conflict for the `n.missing` \
         fixture; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-unknown-field".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let unknown_field_extent = settled
        .extents
        .get("UnknownFieldConflict")
        .expect("brix.type package must declare an UnknownFieldConflict relation");
    assert_eq!(
        unknown_field_extent.len(),
        1,
        "native package must derive exactly one UnknownFieldConflict row"
    );

    let native_conflicts: Vec<TypeConflict> = unknown_field_extent
        .values()
        .map(|record| {
            let resolved = typefacts::resolve_unknown_field(&export.tokens, &record.row).expect(
                "every derived UnknownFieldConflict row's tokens must resolve through the token table",
            );
            TypeConflict {
                subject: resolved.subject,
                kind: ConflictKind::UnknownField {
                    field: resolved.field,
                },
                because: BTreeSet::new(),
                scope: resolved.scope,
            }
        })
        .collect();

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived UnknownField conflict set must canonical-byte-equal reflect.rs's own \
         (same subject, same field name, same scope)"
    );
}

/// #15 native slice 7 (Occurs, `Option` unary-family form): `occurs_check`'s
/// query forces `bind_ty` to attempt `?v := Rel<{value: Option<?v>}>` —
/// `reflect.rs`'s `solve::occurs` rejects it via the `Option` descent arm.
/// The native `OccursDetected` rule must independently reach the same
/// verdict via `TyChild`/`TyReaches` — pure positive recursion, no
/// negation, over the `typefacts::decompose_ty` structure edges the
/// `BindAttempt` export emits for the (already resolved) bind target.
#[test]
fn occurs_check_derives_one_occurs_conflict() {
    let fixture = occurs_check();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts: Vec<&TypeConflict> = report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::Occurs { .. }))
        .collect();
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one Occurs conflict for the occurs_check fixture; \
         got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-occurs".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let occurs_extent = settled
        .extents
        .get("OccursConflict")
        .expect("brix.type package must declare an OccursConflict relation");
    assert_eq!(
        occurs_extent.len(),
        1,
        "native package must derive exactly one OccursConflict row"
    );

    let native_conflicts: Vec<TypeConflict> = occurs_extent
        .values()
        .map(|record| {
            let resolved = typefacts::resolve_occurs(&export.tokens, &record.row).expect(
                "every derived OccursConflict row's tokens must resolve through the token table",
            );
            TypeConflict {
                subject: resolved.subject,
                kind: ConflictKind::Occurs {
                    var: resolved.var,
                    into: resolved.into,
                },
                because: BTreeSet::new(),
                scope: resolved.scope,
            }
        })
        .collect();

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived Occurs conflict set must canonical-byte-equal reflect.rs's own \
         (same subject, same var, same into type, same scope)"
    );
}

/// #15 native slice 7 (Occurs, `Rel` row-descent form): `occurs_check_row`'s
/// query forces `bind_ty` to attempt `?v := Rel<{value: Rel<{inner: ?v}>}>` —
/// a soundness case the `Option`-only `occurs_check` fixture above can't
/// reach, since it never exercises `TyRowChild`/row descent at all. Proves
/// the native `OccursDetected` rule's `TyReaches` closure walks `TyRowChild`
/// edges (via `TyEdgeRow`) exactly as faithfully as it walks `TyChild`
/// (via `TyEdgeApp`).
#[test]
fn occurs_check_row_descent_derives_one_occurs_conflict() {
    let fixture = occurs_check_row();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts: Vec<&TypeConflict> = report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::Occurs { .. }))
        .collect();
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one Occurs conflict for the occurs_check_row fixture; \
         got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-occurs-row".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let occurs_extent = settled
        .extents
        .get("OccursConflict")
        .expect("brix.type package must declare an OccursConflict relation");
    assert_eq!(
        occurs_extent.len(),
        1,
        "native package must derive exactly one OccursConflict row"
    );

    let native_conflicts: Vec<TypeConflict> = occurs_extent
        .values()
        .map(|record| {
            let resolved = typefacts::resolve_occurs(&export.tokens, &record.row).expect(
                "every derived OccursConflict row's tokens must resolve through the token table",
            );
            TypeConflict {
                subject: resolved.subject,
                kind: ConflictKind::Occurs {
                    var: resolved.var,
                    into: resolved.into,
                },
                because: BTreeSet::new(),
                scope: resolved.scope,
            }
        })
        .collect();

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived Occurs conflict set must canonical-byte-equal reflect.rs's own \
         (same subject, same var, same into type, same scope)"
    );
}
