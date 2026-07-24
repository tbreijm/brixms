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
use brix_ir::ident::{Ident, QualIdent};
use brix_ir::reflect::{
    analyze, write_conflict, ConflictKind, Fact, FactId, ReflectiveReport, Subject, TypeConflict,
};
use brix_ir::types::{IntWidth, Ty};
use brix_rt::engine::{Extent, Program, Store, Transaction};
use brixc::pipeline::PhaseAssign;
use brixc::{emit, lower_file, AstPhase};

use brix_conformance::typecorpus::{
    arity_mismatch, arity_non_first_candidate_match_is_not_a_conflict, closed_row_extra_field,
    closed_row_missing_field, container_vs_container_mismatch, container_vs_plain_mismatch,
    cross_epistemic_wrapper_mismatch, estimate_same_ctor_mismatch, estimate_to_plain_erasure,
    estimate_vs_container_erasure, field_failure, flagship_pricing_mutation,
    missing_to_plain_implicit_coercion, occurs_check, occurs_check_row, open_row_extra_field,
    overload_bind_chain, plain_scalar_mismatch, probability_f64_bridge_is_not_a_conflict,
    probability_to_bool_erasure, quantity_add_dimension_mismatch,
    quantity_add_same_dimension_is_not_a_conflict, rule_impure_effect_row,
    rule_mask_ref_not_edge_bound, rule_ordinary_fn_on_derived_rel, rule_unbound_head_key,
    same_container_leaf_no_double_count, subst_chain_composite_root, subst_chain_scalar_root,
    try_non_result, try_over_result_is_not_a_conflict, RuleFixture, NATIVE_GUARD_NON_BOOL_FIXTURE,
    NATIVE_OPERATOR_APPLIES_FIXTURE, NATIVE_ROLE_BINDINGS_FIXTURE,
    NATIVE_ROLE_LIT_MISMATCH_FIXTURE, NATIVE_VAR_SAME_ROLE_TWICE_FIXTURE,
    NATIVE_VAR_THREE_ROLES_FIXTURE, NATIVE_VAR_TWO_ROLES_MISMATCH_FIXTURE,
    NATIVE_WHEN_REQUIRES_BOOL_FIXTURE,
};
use brixc::selfhost::typefacts;

const PACKAGE_SRC: &str = include_str!("../../../packages/brix.type/src/world.brix");

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

/// Wrap a [`RuleFixture`]'s bare `Rule` in a single-rule [`FrontendSource`]
/// and run `reflect::analyze` — the `RuleFixture` counterpart of
/// [`analyze_source`] (a `RuleFixture` carries `rule`/`resolver` directly,
/// not a full `FrontendSource`; mirrors `type_parity.rs`'s
/// `assert_rule_side_condition_parity`).
fn analyze_rule_fixture(fixture: &RuleFixture) -> ReflectiveReport {
    let source = brix_ir::frontend::FrontendSource {
        functions: Vec::new(),
        rules: vec![fixture.rule.clone()],
        constraints: vec![],
        queries: vec![],
    };
    analyze(&source, &fixture.resolver)
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

/// Every `Arity`-kind conflict `reflect::analyze` recorded (native Arity
/// slice) — the arity counterpart of [`reflect_mismatches`].
fn reflect_arity_conflicts(report: &ReflectiveReport) -> Vec<&TypeConflict> {
    report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::Arity { .. }))
        .collect()
}

/// Every `EpistemicErasure`-kind conflict `reflect::analyze` recorded (#15
/// native slice 8) — the erasure counterpart of [`reflect_mismatches`].
fn reflect_erasures(report: &ReflectiveReport) -> Vec<&TypeConflict> {
    report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::EpistemicErasure { .. }))
        .collect()
}

/// Resolve every row of a settled `EpistemicErasureConflict` extent back to a
/// comparable [`TypeConflict`] via the exporter's token table (#15 native
/// slice 8) — the erasure counterpart of [`native_mismatches`].
fn native_erasures(tokens: &typefacts::TokenTable, extent: &Extent) -> Vec<TypeConflict> {
    extent
        .values()
        .map(|record| {
            let resolved = typefacts::resolve_epistemic_erasure(tokens, &record.row).expect(
                "every derived EpistemicErasureConflict row's tokens must resolve through the token table",
            );
            TypeConflict {
                subject: resolved.subject,
                kind: ConflictKind::EpistemicErasure {
                    from: resolved.from,
                    to: resolved.to,
                },
                because: BTreeSet::new(),
                scope: resolved.scope,
            }
        })
        .collect()
}

/// Every `Dimension`-kind conflict `reflect::analyze` recorded (native
/// Dimension slice, add/sub same-dimension) — the Dimension counterpart of
/// [`reflect_mismatches`]/[`reflect_arity_conflicts`].
fn reflect_dimension_conflicts(report: &ReflectiveReport) -> Vec<&TypeConflict> {
    report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::Dimension { .. }))
        .collect()
}

/// Resolve every row of a settled `DimensionConflict` extent back to a
/// comparable [`TypeConflict`] via the exporter's token table (native
/// Dimension slice) — the Dimension counterpart of
/// [`native_mismatches`]/[`native_erasures`].
fn native_dimensions(tokens: &typefacts::TokenTable, extent: &Extent) -> Vec<TypeConflict> {
    extent
        .values()
        .map(|record| {
            let resolved = typefacts::resolve_dimension(tokens, &record.row).expect(
                "every derived DimensionConflict row's tokens must resolve through the token table",
            );
            TypeConflict {
                subject: resolved.subject,
                kind: ConflictKind::Dimension {
                    op: resolved.op,
                    left: resolved.left,
                    right: resolved.right,
                },
                because: BTreeSet::new(),
                scope: resolved.scope,
            }
        })
        .collect()
}

/// Every `ImpureRule`-kind conflict `reflect::analyze` recorded (#15 native
/// rule-side-conditions slice) — the ImpureRule counterpart of
/// [`reflect_mismatches`]/[`reflect_arity_conflicts`].
fn reflect_impure_rule_conflicts(report: &ReflectiveReport) -> Vec<&TypeConflict> {
    report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::ImpureRule))
        .collect()
}

/// Resolve every row of a settled `ImpureRuleConflict` extent back to a
/// comparable [`TypeConflict`] via the exporter's token table — the native
/// counterpart of [`reflect_impure_rule_conflicts`].
fn native_impure_rule_conflicts(
    tokens: &typefacts::TokenTable,
    extent: &Extent,
) -> Vec<TypeConflict> {
    extent
        .values()
        .map(|record| {
            let resolved = typefacts::resolve_rule_impure(tokens, &record.row).expect(
                "every derived ImpureRuleConflict row's tokens must resolve through the token table",
            );
            TypeConflict {
                subject: resolved.subject,
                kind: ConflictKind::ImpureRule,
                because: BTreeSet::new(),
                scope: resolved.scope,
            }
        })
        .collect()
}

/// Every `UnboundHeadKey`-kind conflict `reflect::analyze` recorded (#15
/// native rule-side-conditions slice) — the UnboundHeadKey counterpart of
/// [`reflect_mismatches`]/[`reflect_arity_conflicts`].
fn reflect_unbound_head_key_conflicts(report: &ReflectiveReport) -> Vec<&TypeConflict> {
    report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::UnboundHeadKey { .. }))
        .collect()
}

/// Resolve every row of a settled `UnboundHeadKeyConflict` extent back to a
/// comparable [`TypeConflict`] via the exporter's token table — the native
/// counterpart of [`reflect_unbound_head_key_conflicts`].
fn native_unbound_head_key_conflicts(
    tokens: &typefacts::TokenTable,
    extent: &Extent,
) -> Vec<TypeConflict> {
    extent
        .values()
        .map(|record| {
            let resolved = typefacts::resolve_unbound_head_key(tokens, &record.row).expect(
                "every derived UnboundHeadKeyConflict row's tokens must resolve through the token table",
            );
            TypeConflict {
                subject: resolved.subject,
                kind: ConflictKind::UnboundHeadKey { key: resolved.key },
                because: BTreeSet::new(),
                scope: resolved.scope,
            }
        })
        .collect()
}

/// Every `MaskRefNotEdgeBound`-kind conflict `reflect::analyze` recorded
/// (#15 native rule-side-conditions slice) — the MaskRefNotEdgeBound
/// counterpart of [`reflect_mismatches`]/[`reflect_arity_conflicts`].
fn reflect_mask_ref_conflicts(report: &ReflectiveReport) -> Vec<&TypeConflict> {
    report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::MaskRefNotEdgeBound { .. }))
        .collect()
}

/// Resolve every row of a settled `MaskRefNotEdgeBoundConflict` extent back
/// to a comparable [`TypeConflict`] via the exporter's token table — the
/// native counterpart of [`reflect_mask_ref_conflicts`].
fn native_mask_ref_conflicts(tokens: &typefacts::TokenTable, extent: &Extent) -> Vec<TypeConflict> {
    extent
        .values()
        .map(|record| {
            let resolved = typefacts::resolve_mask_ref(tokens, &record.row).expect(
                "every derived MaskRefNotEdgeBoundConflict row's tokens must resolve through the token table",
            );
            TypeConflict {
                subject: resolved.subject,
                kind: ConflictKind::MaskRefNotEdgeBound { var: resolved.var },
                because: BTreeSet::new(),
                scope: resolved.scope,
            }
        })
        .collect()
}

/// Every `OrdinaryFnOnDerivedRel`-kind conflict `reflect::analyze` recorded
/// (#15 native rule-side-conditions slice) — the OrdinaryFnOnDerivedRel
/// counterpart of [`reflect_mismatches`]/[`reflect_arity_conflicts`].
fn reflect_ordinary_fn_conflicts(report: &ReflectiveReport) -> Vec<&TypeConflict> {
    report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::OrdinaryFnOnDerivedRel { .. }))
        .collect()
}

/// Resolve every row of a settled `OrdinaryFnOnDerivedRelConflict` extent
/// back to a comparable [`TypeConflict`] via the exporter's token table —
/// the native counterpart of [`reflect_ordinary_fn_conflicts`].
fn native_ordinary_fn_conflicts(
    tokens: &typefacts::TokenTable,
    extent: &Extent,
) -> Vec<TypeConflict> {
    extent
        .values()
        .map(|record| {
            let resolved = typefacts::resolve_ordinary_fn(tokens, &record.row).expect(
                "every derived OrdinaryFnOnDerivedRelConflict row's tokens must resolve through the token table",
            );
            TypeConflict {
                subject: resolved.subject,
                kind: ConflictKind::OrdinaryFnOnDerivedRel {
                    relation: resolved.relation,
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

/// #15 native slice 8 (`step` classification, flat Mismatch): `reflect.rs`'s
/// own `unify` recursion reaches the `Bool`-vs-`Int` leaf through Query
/// result-vs-yields *row descent* (`unify_rows` → leaf `unify`), never
/// through `role_arg`'s direct literal compare — so this is the first
/// selfhost fixture only `UnifyMismatch` reproduces; neither
/// `LitRoleMismatch` nor `VarRoleMismatch` fires on it.
#[test]
fn plain_scalar_mismatch_derives_exactly_one_mismatch_conflict() {
    let fixture = plain_scalar_mismatch();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts = reflect_mismatches(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one Mismatch conflict for the plain_scalar_mismatch \
         fixture; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-plain-scalar-mismatch".to_vec());
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
        "native package must derive exactly one MismatchConflict row"
    );

    let native_conflicts = native_mismatches(&export.tokens, mismatch_extent);

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived MismatchConflict set must be canonical-byte-identical \
         to reflect.rs's own conflict set"
    );
}

/// #15 native slice 8 (`step` classification, `Probability`/`F64` bridge):
/// the deliberately-kept v1 bridge (solve.rs:163) is `Step::Done` — proves it
/// is a non-event both in `reflect.rs` and, end-to-end, in the native
/// package: zero `MismatchConflict` rows and zero `EpistemicErasureConflict`
/// rows, matching reflect's own zero Mismatch and zero EpistemicErasure
/// conflicts. Distinct from `probability_to_bool_erasure`, the OTHER "plain"
/// partner, which IS a named erasure.
#[test]
fn probability_f64_bridge_is_a_non_event_end_to_end() {
    let fixture = probability_f64_bridge_is_not_a_conflict();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_mismatch_conflicts = reflect_mismatches(&report);
    assert!(
        reflect_mismatch_conflicts.is_empty(),
        "reflect.rs must record zero Mismatch conflicts for the Probability/F64 bridge \
         fixture; got {reflect_mismatch_conflicts:?}"
    );
    let reflect_erasure_conflicts = reflect_erasures(&report);
    assert!(
        reflect_erasure_conflicts.is_empty(),
        "reflect.rs must record zero EpistemicErasure conflicts for the Probability/F64 \
         bridge fixture; got {reflect_erasure_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-probability-f64-bridge".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    // An empty extent may be absent from `settled.extents` entirely or
    // present with length 0 — handle both, since the bridge fixture never
    // asserts a single `MismatchConflict`/`EpistemicErasureConflict` row.
    let mismatch_len = settled
        .extents
        .get("MismatchConflict")
        .map_or(0, |extent| extent.len());
    assert_eq!(
        mismatch_len, 0,
        "native package must derive zero MismatchConflict rows for the Probability/F64 bridge"
    );
    let erasure_len = settled
        .extents
        .get("EpistemicErasureConflict")
        .map_or(0, |extent| extent.len());
    assert_eq!(
        erasure_len, 0,
        "native package must derive zero EpistemicErasureConflict rows for the \
         Probability/F64 bridge"
    );
}

/// #15 native slice 8 (`step` classification, epistemic Erasure —
/// `Estimate<T>`): mirrors `estimate_to_plain_erasure`'s reference conflict
/// (`Estimate<Int>` unified against `Int`) against the native
/// `EstimateErasureFwd`/`EstimateErasureBwd` rules.
#[test]
fn estimate_to_plain_erasure_derives_exactly_one_erasure_conflict() {
    let fixture = estimate_to_plain_erasure();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts = reflect_erasures(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one EpistemicErasure conflict for the \
         estimate_to_plain_erasure fixture; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-estimate-erasure".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let erasure_extent = settled
        .extents
        .get("EpistemicErasureConflict")
        .expect("brix.type package must declare an EpistemicErasureConflict relation");
    assert_eq!(
        erasure_extent.len(),
        1,
        "native package must derive exactly one EpistemicErasureConflict row"
    );

    let native_conflicts = native_erasures(&export.tokens, erasure_extent);

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived EpistemicErasureConflict set must canonical-byte-equal \
         reflect.rs's own (same subject, same from/to types, same scope)"
    );
}

/// #15 native slice 8 (`step` classification, epistemic Erasure —
/// `Probability`/`Bool`): mirrors `probability_to_bool_erasure`'s reference
/// conflict against the native `ProbabilityBoolErasureFwd`/
/// `ProbabilityBoolErasureBwd` rules — distinct from the `Probability`/`F64`
/// bridge (see `probability_f64_bridge_is_a_non_event_end_to_end`), which
/// must NOT derive an erasure.
#[test]
fn probability_to_bool_erasure_derives_exactly_one_erasure_conflict() {
    let fixture = probability_to_bool_erasure();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts = reflect_erasures(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one EpistemicErasure conflict for the \
         probability_to_bool_erasure fixture; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-probability-bool-erasure".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let erasure_extent = settled
        .extents
        .get("EpistemicErasureConflict")
        .expect("brix.type package must declare an EpistemicErasureConflict relation");
    assert_eq!(
        erasure_extent.len(),
        1,
        "native package must derive exactly one EpistemicErasureConflict row"
    );

    let native_conflicts = native_erasures(&export.tokens, erasure_extent);

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived EpistemicErasureConflict set must canonical-byte-equal \
         reflect.rs's own (same subject, same from/to types, same scope)"
    );
}

/// #15 native slice 8 (`step` classification, epistemic Erasure —
/// `Missing<T>`): mirrors `missing_to_plain_implicit_coercion`'s reference
/// conflict against the native `MissingErasureFwd`/`MissingErasureBwd`
/// rules.
#[test]
fn missing_to_plain_implicit_coercion_derives_exactly_one_erasure_conflict() {
    let fixture = missing_to_plain_implicit_coercion();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts = reflect_erasures(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one EpistemicErasure conflict for the \
         missing_to_plain_implicit_coercion fixture; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-missing-erasure".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let erasure_extent = settled
        .extents
        .get("EpistemicErasureConflict")
        .expect("brix.type package must declare an EpistemicErasureConflict relation");
    assert_eq!(
        erasure_extent.len(),
        1,
        "native package must derive exactly one EpistemicErasureConflict row"
    );

    let native_conflicts = native_erasures(&export.tokens, erasure_extent);

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived EpistemicErasureConflict set must canonical-byte-equal \
         reflect.rs's own (same subject, same from/to types, same scope)"
    );
}

/// #15 native slice 9 (binding fixpoint): reflect's own fully-chased ground
/// truth for one fixture, restricted to vars that were actually inserted into
/// `subst` (accepted binds only — via `Fact::SubstEdge`). A rejected bind (an
/// occurs-check failure) has a `BindAttempt` but no `SubstEdge`, and produces
/// no native `Resolved` row either, so filtering keeps this helper correct
/// even for fixtures that mix accepted and rejected binds.
fn reflect_resolved(report: &ReflectiveReport) -> BTreeSet<(brix_ir::types::TyVar, Ty)> {
    // vars that were actually inserted into subst (accepted binds)
    let accepted: BTreeSet<brix_ir::types::TyVar> = report
        .facts
        .iter()
        .filter_map(|d| match &d.fact {
            Fact::SubstEdge { var, .. } => Some(*var),
            _ => None,
        })
        .collect();
    report
        .facts
        .iter()
        .filter_map(|d| match &d.fact {
            Fact::BindAttempt { var, target, .. } if accepted.contains(var) => {
                Some((*var, target.clone()))
            }
            _ => None,
        })
        .collect()
}

/// #15 native slice 9 (binding fixpoint, scalar root): proves the native
/// `Bound`/`Resolved` fixpoint reproduces `solve::resolve`'s own transitive
/// chase over the RAW, un-chased `SubstEdge` edges — `?a := ?b` then
/// `?b := Int` (subst `{a: Var(b), b: Int}`), chased to `a -> Int`, `b -> Int`.
#[test]
fn subst_chain_scalar_root_resolves_two_hops_to_the_same_final_type() {
    let fixture = subst_chain_scalar_root();
    let report = analyze(&fixture.source, &fixture.resolver);

    let expected = reflect_resolved(&report);
    assert_eq!(
        expected.len(),
        2,
        "reflect must accept both binds (A and B) in this fixture; got {expected:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-subst-chain-scalar".to_vec());
    txn.ops = export.ops;
    let mut store = Store::new(compiled_package());
    let settled = store.commit(&txn).expect("shadow mode never rejects");

    let resolved_extent = settled
        .extents
        .get("Resolved")
        .expect("brix.type package must declare a Resolved relation");
    assert_eq!(
        resolved_extent.len(),
        2,
        "native must derive exactly two Resolved rows"
    );

    let native: BTreeSet<(brix_ir::types::TyVar, Ty)> = resolved_extent
        .values()
        .map(|record| {
            let r = typefacts::resolve_resolved(&export.tokens, &record.row)
                .expect("every derived Resolved row's tokens must resolve through the token table");
            (r.var, r.root)
        })
        .collect();

    assert_eq!(
        native, expected,
        "native Resolved(var, root) must equal reflect's own fully-chased BindAttempt.target \
         for every accepted var — proving the native fixpoint reproduces solve::resolve's chase"
    );
}

/// #15 native slice 9 (binding fixpoint, composite root): [`subst_chain_scalar_root`]'s
/// counterpart chasing to a pre-tokenized ground composite root (`Option<Int>`)
/// instead of a bare scalar, proving the fixpoint doesn't special-case the
/// terminal shape — it only ever inspects `TyCtorIs`, which the composite
/// root's own token gets exactly once, same as any other operand.
#[test]
fn subst_chain_composite_root_resolves_to_the_composite() {
    let fixture = subst_chain_composite_root();
    let report = analyze(&fixture.source, &fixture.resolver);

    let expected = reflect_resolved(&report);
    assert_eq!(
        expected.len(),
        2,
        "reflect must accept both binds (A and B) in this fixture; got {expected:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-subst-chain-composite".to_vec());
    txn.ops = export.ops;
    let mut store = Store::new(compiled_package());
    let settled = store.commit(&txn).expect("shadow mode never rejects");

    let resolved_extent = settled
        .extents
        .get("Resolved")
        .expect("brix.type package must declare a Resolved relation");
    assert_eq!(
        resolved_extent.len(),
        2,
        "native must derive exactly two Resolved rows"
    );

    let native: BTreeSet<(brix_ir::types::TyVar, Ty)> = resolved_extent
        .values()
        .map(|record| {
            let r = typefacts::resolve_resolved(&export.tokens, &record.row)
                .expect("every derived Resolved row's tokens must resolve through the token table");
            (r.var, r.root)
        })
        .collect();

    assert_eq!(
        native, expected,
        "native Resolved(var, root) must equal reflect's own fully-chased BindAttempt.target \
         for every accepted var — proving the native fixpoint reproduces solve::resolve's chase \
         even when the chain terminates in a composite (non-leaf) root"
    );
}

/// #15 gap D: proves the overloaded-call commit site (reflect.rs:1719, formerly
/// a wholesale `self.subst = next`) now emits BindAttempt/SubstEdge for the var
/// it binds, closing the one genuinely unsound gap. Layer 1 asserts on
/// report.facts that the CALL's own bind (var 9400 = A) produced a SubstEdge —
/// the pre-fix/post-fix discriminator (reflect_resolved reads the same fact
/// stream the gap starves, so its equality alone would pass vacuously pre-fix).
/// Layer 2 is the subst_chain_scalar_root native-parity shape.
#[test]
fn overload_bind_chain_resolves_two_hops_to_the_same_final_type() {
    let fixture = overload_bind_chain();
    let report = analyze(&fixture.source, &fixture.resolver);

    let call_bind_present = report.facts.iter().any(|d| {
        matches!(&d.fact, Fact::SubstEdge { var, target: Ty::Var(_), .. }
            if *var == brix_ir::types::TyVar(9400))
    });
    assert!(
        call_bind_present,
        "reflect.rs must record a SubstEdge for the var the overloaded call itself \
         binds (var 9400, `A := Var(B)`) — the fact reflect.rs previously swallowed \
         at the `self.subst = next` commit"
    );

    let expected = reflect_resolved(&report);
    assert_eq!(
        expected.len(),
        2,
        "reflect must accept both binds (call-bound A and query-result-bound B); got {expected:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-overload-bind-chain".to_vec());
    txn.ops = export.ops;
    let mut store = Store::new(compiled_package());
    let settled = store.commit(&txn).expect("shadow mode never rejects");

    let resolved_extent = settled
        .extents
        .get("Resolved")
        .expect("brix.type package must declare a Resolved relation");
    assert_eq!(
        resolved_extent.len(),
        2,
        "native must derive exactly two Resolved rows"
    );

    let native: BTreeSet<(brix_ir::types::TyVar, Ty)> = resolved_extent
        .values()
        .map(|record| {
            let r = typefacts::resolve_resolved(&export.tokens, &record.row)
                .expect("every derived Resolved row's tokens must resolve through the token table");
            (r.var, r.root)
        })
        .collect();

    assert_eq!(
        native, expected,
        "native Resolved(var, root) must equal reflect's own fully-chased BindAttempt.target \
         for every accepted var, including the var bound by the overloaded-call commit path"
    );
}

/// #15 gap-closure (slice 8 B+C, Gap B — container vs a DIFFERENT ctor):
/// `Result<Bool, Str>` vs `Option<Int>`, reached through the `value`-field row
/// descent (both top-level `Rel` wrappers agree, so `step` `Rows`-descends into
/// the field, where the two DIFFERENT container ctors flatten straight to
/// `Mismatch`). Only `UnifyMismatchCrossCtor` reproduces this.
#[test]
fn container_vs_container_derives_exactly_one_mismatch_conflict() {
    let fixture = container_vs_container_mismatch();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts = reflect_mismatches(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one Mismatch conflict for the \
         container_vs_container_mismatch fixture; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-container-vs-container".to_vec());
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
        "native package must derive exactly one MismatchConflict row"
    );

    let native_conflicts = native_mismatches(&export.tokens, mismatch_extent);

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived MismatchConflict set must be canonical-byte-identical \
         to reflect.rs's own conflict set"
    );
}

/// #15 gap-closure (slice 8 B+C, Gap B — container vs plain): `Option<Int>`
/// vs a bare `Int` — different ctors, `step` flattens to `Mismatch`.
/// `UnifyMismatch` doesn't reach this (`Option` isn't `TyCtorOrdinary`); only
/// `UnifyMismatchCrossCtor` does.
#[test]
fn container_vs_plain_derives_exactly_one_mismatch_conflict() {
    let fixture = container_vs_plain_mismatch();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts = reflect_mismatches(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one Mismatch conflict for the \
         container_vs_plain_mismatch fixture; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-container-vs-plain".to_vec());
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
        "native package must derive exactly one MismatchConflict row"
    );

    let native_conflicts = native_mismatches(&export.tokens, mismatch_extent);

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived MismatchConflict set must be canonical-byte-identical \
         to reflect.rs's own conflict set"
    );
}

/// #15 gap-closure (slice 8 B+C, Gap B — epistemic vs container erasure):
/// `Estimate<Int>` vs `Option<Int>` — `is_plain` (solve.rs:263) is TRUE for
/// containers, so this is a genuine `Erasure`. Only the amended
/// `EstimateErasureFwd`/`Bwd` (joining the broadened `TyCtorPlain` set)
/// reproduce it.
#[test]
fn estimate_vs_container_derives_exactly_one_erasure_conflict() {
    let fixture = estimate_vs_container_erasure();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts = reflect_erasures(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one EpistemicErasure conflict for the \
         estimate_vs_container_erasure fixture; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-estimate-vs-container".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let erasure_extent = settled
        .extents
        .get("EpistemicErasureConflict")
        .expect("brix.type package must declare an EpistemicErasureConflict relation");
    assert_eq!(
        erasure_extent.len(),
        1,
        "native package must derive exactly one EpistemicErasureConflict row"
    );

    let native_conflicts = native_erasures(&export.tokens, erasure_extent);

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived EpistemicErasureConflict set must canonical-byte-equal \
         reflect.rs's own (same subject, same from/to types, same scope)"
    );
}

/// #15 gap-closure (slice 8 B+C, Gap C — cross-epistemic-wrapper mismatch):
/// `Estimate<Int>` vs `Missing<Int>` — two DIFFERENT epistemic wrappers;
/// `solve::epistemic_erasure` requires one side `is_plain`, and neither is,
/// so `step` flattens to an ordinary `Mismatch`, never an erasure. Only
/// `UnifyMismatchCrossCtor` reproduces this.
#[test]
fn cross_epistemic_wrapper_derives_exactly_one_mismatch_conflict() {
    let fixture = cross_epistemic_wrapper_mismatch();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts = reflect_mismatches(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one Mismatch conflict for the \
         cross_epistemic_wrapper_mismatch fixture; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-cross-epistemic-wrapper".to_vec());
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
        "native package must derive exactly one MismatchConflict row"
    );

    let native_conflicts = native_mismatches(&export.tokens, mismatch_extent);

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived MismatchConflict set must be canonical-byte-identical \
         to reflect.rs's own conflict set"
    );
}

/// #15 gap-closure (slice 8 B+C, third cell — Estimate vs Estimate): `step`
/// has NO `(Estimate, Estimate)` Descend arm, so `Estimate<Bool>` vs
/// `Estimate<Int>` falls to the catch-all, where `epistemic_erasure` returns
/// `None` — an ordinary `Mismatch` at the container level, no leaf descent.
/// Only the dedicated `EstimateSameCtorMismatch` rule reproduces this.
#[test]
fn estimate_same_ctor_derives_exactly_one_mismatch_conflict() {
    let fixture = estimate_same_ctor_mismatch();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts = reflect_mismatches(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one Mismatch conflict for the \
         estimate_same_ctor_mismatch fixture; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-estimate-same-ctor".to_vec());
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
        "native package must derive exactly one MismatchConflict row"
    );

    let native_conflicts = native_mismatches(&export.tokens, mismatch_extent);

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived MismatchConflict set must be canonical-byte-identical \
         to reflect.rs's own conflict set"
    );
}

/// #15 gap-closure (slice 8 B+C, REGRESSION GUARD — the load-bearing `lc !=
/// rc` guard): `Option<Bool>` vs `Option<Int>` — SAME ctor at the container
/// level, which `step`'s `Option`/`Option` arm `Descend`s into the leaf
/// `Bool` vs `Int` pair, the actual `Mismatch`. This test proves
/// `UnifyMismatchCrossCtor` does NOT also fire at the container level (which
/// would double-count the conflict): exactly ONE `MismatchConflict` results,
/// and — beyond the count/byte-set check every other fixture in this file
/// makes — its resolved `(expect, found)` is asserted to be the LEAF pair
/// `(Bool, Int)`, never the container pair `(Option<Bool>, Option<Int>)`.
#[test]
fn same_container_leaf_derives_exactly_one_mismatch_at_the_leaf_not_the_container() {
    let fixture = same_container_leaf_no_double_count();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts = reflect_mismatches(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one Mismatch conflict for the \
         same_container_leaf_no_double_count fixture (the leaf Bool-vs-Int pair, \
         not the container-level Option-vs-Option pair, which step Descends \
         rather than flags as a Mismatch); got {reflect_conflicts:?}"
    );
    assert_eq!(
        reflect_conflicts[0].kind,
        ConflictKind::Mismatch {
            left: Ty::Bool,
            right: Ty::Int(IntWidth::Int),
        },
        "reflect's own single Mismatch conflict must be the leaf (Bool, Int) pair"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-same-container-leaf".to_vec());
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
        "native package must derive exactly one MismatchConflict row — NOT two \
         (which would mean UnifyMismatchCrossCtor double-counted the same-ctor \
         container pair alongside the leaf pair)"
    );

    let record = mismatch_extent.values().next().unwrap();
    let resolved = typefacts::resolve_mismatch(&export.tokens, &record.row)
        .expect("the derived MismatchConflict row's tokens must resolve through the token table");
    assert_eq!(
        (resolved.expect, resolved.found),
        (Ty::Bool, Ty::Int(IntWidth::Int)),
        "the single native-derived MismatchConflict must resolve to the LEAF pair \
         (Bool, Int), never the container pair (Option<Bool>, Option<Int>) — the \
         exact case the `lc != rc` guard on UnifyMismatchCrossCtor exists to prevent"
    );

    let native_conflicts = native_mismatches(&export.tokens, mismatch_extent);
    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived MismatchConflict set must be canonical-byte-identical \
         to reflect.rs's own conflict set"
    );
}

/// #15 gap-closure A (row-unification UnknownField, `missing_in_left`
/// direction): `query.result` (expect/left) declares a CLOSED `{a}` row but
/// the yielded record (found/right) is `{a, b}` — `solve::match_rows`'s
/// `missing_in_left` set (solve.rs:308-312) fires for field `b`, since `left`
/// is Closed and lacks a counterpart for `right`'s `b`. Proves
/// `UnifyRowsMissingInLeft` end-to-end, gated on `RowClosed(ty: expect)`.
#[test]
fn closed_row_extra_field_derives_one_unknown_field_conflict() {
    let fixture = closed_row_extra_field();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts: Vec<&TypeConflict> = report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::UnknownField { .. }))
        .collect();
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one UnknownField conflict for the \
         closed_row_extra_field fixture; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-closed-row-extra-field".to_vec());
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
        "native package must derive exactly one UnknownFieldConflict row (the \
         UnifyRowsMissingInLeft direction, field `b`)"
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

/// #15 gap-closure A (row-unification UnknownField, `missing_in_right`
/// direction): `closed_row_missing_field`'s mirror of the fixture above —
/// `query.result` (expect/left) declares a CLOSED `{a, b}` row while the
/// yielded record (found/right) only has `{a}` — `solve::match_rows`'s
/// `missing_in_right` set (solve.rs:302-304) fires for field `b`, since
/// `right` is Closed and lacks a counterpart for `left`'s `b`. Proves
/// `UnifyRowsMissingInRight` end-to-end, gated on `RowClosed(ty: found)`.
#[test]
fn closed_row_missing_field_derives_one_unknown_field_conflict() {
    let fixture = closed_row_missing_field();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts: Vec<&TypeConflict> = report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::UnknownField { .. }))
        .collect();
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one UnknownField conflict for the \
         closed_row_missing_field fixture; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-closed-row-missing-field".to_vec());
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
        "native package must derive exactly one UnknownFieldConflict row (the \
         UnifyRowsMissingInRight direction, field `b`)"
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

/// #15 gap-closure A discriminator: `open_row_extra_field` is the closed/open
/// control — the same extra-field shape as `closed_row_extra_field`, but
/// `query.result` declares an OPEN row, so row polymorphism admits the extra
/// field and NEITHER `UnifyRowsMissingInLeft` nor `UnifyRowsMissingInRight`
/// may fire. An inverted `RowClosed` gate would wrongly fire here — this test
/// is what catches that.
#[test]
fn open_row_extra_field_derives_no_unknown_field_conflict() {
    let fixture = open_row_extra_field();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts: Vec<&TypeConflict> = report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::UnknownField { .. }))
        .collect();
    assert!(
        reflect_conflicts.is_empty(),
        "reflect.rs must record zero UnknownField conflicts for the open_row_extra_field \
         fixture; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-open-row-extra-field".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    // An empty extent may be absent from `settled.extents` entirely or
    // present with length 0 — handle both, matching the Probability/F64
    // bridge test's pattern.
    let unknown_field_len = settled
        .extents
        .get("UnknownFieldConflict")
        .map_or(0, |extent| extent.len());
    assert_eq!(
        unknown_field_len, 0,
        "native package must derive zero UnknownFieldConflict rows for the open row — \
         an inverted RowClosed gate would wrongly fire here"
    );
}

/// #15 native slice-11 (TryNonResult): a `?` postfix applied to a non-`Result`
/// (`Int`) value. `reflect.rs`'s `ExprKind::Try` arm raises
/// `ConflictKind::TryNonResult{found: Int}`. The native package reproduces
/// this from the imported `TryExpr`/`ExprChild` structural facts joined
/// against the (now `TyCtorIs`-seeded) `ExprType` of the inner subject, via
/// `TryInnerOf`/`TryNonResultCheck`.
#[test]
fn try_non_result_derives_one_conflict() {
    let fixture = try_non_result();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts: Vec<&TypeConflict> = report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::TryNonResult { .. }))
        .collect();
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one TryNonResult conflict for the try_non_result \
         fixture; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-try-non-result".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let try_non_result_extent = settled
        .extents
        .get("TryNonResultConflict")
        .expect("brix.type package must declare a TryNonResultConflict relation");
    assert_eq!(
        try_non_result_extent.len(),
        1,
        "native package must derive exactly one TryNonResultConflict row"
    );

    let native_conflicts: Vec<TypeConflict> = try_non_result_extent
        .values()
        .map(|record| {
            let resolved = typefacts::resolve_try_non_result(&export.tokens, &record.row).expect(
                "every derived TryNonResultConflict row's tokens must resolve through the token table",
            );
            TypeConflict {
                subject: resolved.subject,
                kind: ConflictKind::TryNonResult {
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
        "the native-derived TryNonResult conflict set must canonical-byte-equal reflect.rs's own \
         (same subject, same `found` type, same scope)"
    );
}

/// #15 native slice-11 discriminator: a `?` postfix applied to a genuine
/// `Result` value — the control for `try_non_result_derives_one_conflict`,
/// proving `TryNonResultCheck`'s `when ctor != 9` guard doesn't over-fire.
#[test]
fn try_over_result_derives_no_conflict() {
    let fixture = try_over_result_is_not_a_conflict();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts: Vec<&TypeConflict> = report
        .conflicts
        .iter()
        .filter(|conflict| matches!(conflict.kind, ConflictKind::TryNonResult { .. }))
        .collect();
    assert!(
        reflect_conflicts.is_empty(),
        "reflect.rs must record zero TryNonResult conflicts for the try_over_result_is_not_a_conflict \
         fixture; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-try-over-result".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    // An empty extent may be absent from `settled.extents` entirely or
    // present with length 0 — handle both, matching the Probability/F64
    // bridge test's pattern.
    let try_non_result_len = settled
        .extents
        .get("TryNonResultConflict")
        .map_or(0, |extent| extent.len());
    assert_eq!(
        try_non_result_len, 0,
        "native package must derive zero TryNonResultConflict rows for a try over a genuine Result"
    );
}

/// Native Arity slice: `arity_mismatch`'s single-candidate call — `f(Int) ->
/// Int` called with zero args — where `reflect.rs`'s `arity_ok.is_empty()`
/// branch fires, recording `ConflictKind::Arity{expected: 1, found: 0}`. The
/// native `CallArityMismatch` rule must reproduce the identical conflict via
/// `OpApply` ⋈ `CallArity` ⋈ `FnArity(ordinal 0)` ⋈ `RootScope`, gated by the
/// absence of ANY candidate (fresh `any_ord`) whose `paramc` equals `found`.
#[test]
fn arity_mismatch_derives_one_arity_conflict() {
    let fixture = arity_mismatch();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts = reflect_arity_conflicts(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one Arity conflict for the arity_mismatch \
         fixture; got {reflect_conflicts:?}"
    );
    assert_eq!(
        reflect_conflicts[0].kind,
        ConflictKind::Arity {
            expected: 1,
            found: 0,
        },
        "reflect's Arity conflict must be expected:1 (f's declared arity), found:0 \
         (the call's actual arg count)"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-arity-mismatch".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let arity_extent = settled
        .extents
        .get("ArityConflict")
        .expect("brix.type package must declare an ArityConflict relation");
    assert_eq!(
        arity_extent.len(),
        1,
        "native package must derive exactly one ArityConflict row"
    );

    let native_conflicts: Vec<TypeConflict> = arity_extent
        .values()
        .map(|record| {
            let resolved = typefacts::resolve_arity(&export.tokens, &record.row).expect(
                "every derived ArityConflict row's tokens must resolve through the token table",
            );
            TypeConflict {
                subject: resolved.subject,
                kind: ConflictKind::Arity {
                    expected: resolved.expected,
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
        "the native-derived ArityConflict set must canonical-byte-equal reflect.rs's own \
         (same subject, same expected/found counts, same scope)"
    );
}

/// Native Arity slice discriminator: two overloads of `g` (`g(Int)`, ordinal
/// 0; `g(Int, Int)`, ordinal 1), called with two `Int` args — matching ONLY
/// the ordinal-1 candidate. `reflect.rs`'s `arity_ok` filter keeps that
/// candidate, so reflect records ZERO Arity conflicts. This is the fresh-
/// `any_ord`-correctness proof: a native `CallArityMismatch` `without` block
/// that wrongly reused the ordinal-0 literal (instead of ranging over every
/// candidate) would only check candidate 0's paramc (1) against found (2),
/// see no match, and wrongly derive `ArityConflict{expected:1, found:2}`.
#[test]
fn arity_non_first_candidate_match_derives_zero_arity_conflicts() {
    let fixture = arity_non_first_candidate_match_is_not_a_conflict();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts = reflect_arity_conflicts(&report);
    assert!(
        reflect_conflicts.is_empty(),
        "reflect.rs must record zero Arity conflicts when a later overload's \
         arity matches the call; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-arity-non-first-candidate".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let arity_len = settled
        .extents
        .get("ArityConflict")
        .map_or(0, |extent| extent.len());
    assert_eq!(
        arity_len, 0,
        "native package must derive zero ArityConflict rows — a `without` block that \
         reused the ordinal-0 literal instead of a fresh existential var would wrongly \
         fire here"
    );
}

/// Native Dimension slice (add/sub same-dimension): `quantity_add_dimension_mismatch`'s
/// minimal `add(a, b)` over two GROUND, UNEQUAL dimensioned operands
/// (`Quantity(Mass)`, `Quantity(Kilometre)`) — `reflect.rs`'s `same_dimension`
/// records `ConflictKind::Dimension{op:"add", left, right}` directly (no
/// mul/div dimension arithmetic in the way). The native `DimSameMismatch`
/// rule must independently reach the same verdict, deciding the conflict
/// itself from the `TyDims` digest inequality rather than restating
/// reflect's own conclusion — proven by the discriminator immediately below.
#[test]
fn quantity_add_dimension_mismatch_derives_one_dimension_conflict() {
    let fixture = quantity_add_dimension_mismatch();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts = reflect_dimension_conflicts(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one Dimension conflict for the \
         quantity_add_dimension_mismatch fixture; got {reflect_conflicts:?}"
    );
    assert_eq!(
        reflect_conflicts[0].kind,
        ConflictKind::Dimension {
            op: "add".to_string(),
            left: Ty::Quantity(Ident::new("Mass")),
            right: Ty::Quantity(Ident::new("Kilometre")),
        },
        "reflect's Dimension conflict must be op:\"add\", left: Quantity(Mass), \
         right: Quantity(Kilometre) — the verbatim operands, ground and unequal"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-quantity-add-dimension-mismatch".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let dimension_extent = settled
        .extents
        .get("DimensionConflict")
        .expect("brix.type package must declare a DimensionConflict relation");
    assert_eq!(
        dimension_extent.len(),
        1,
        "native package must derive exactly one DimensionConflict row"
    );

    let native_conflicts = native_dimensions(&export.tokens, dimension_extent);

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived DimensionConflict set must be canonical-byte-identical \
         to reflect.rs's own conflict set (same subject, same op, same left/right, same scope)"
    );
}

/// Native Dimension slice discriminator: `quantity_add_same_dimension_is_not_a_conflict`'s
/// `add(a, b)` over two operands sharing the SAME ground dimension
/// (`Quantity(Kilometre)` + `Quantity(Kilometre)`) — `same_dimension_step`'s
/// `x == y` arm returns `Ok`, so reflect raises ZERO Dimension conflicts.
/// The native package must independently reach zero `DimensionConflict` rows
/// too — not because it saw nothing (`DimOp`/`TyDims` rows DO exist for this
/// query), but because `DimSameMismatch`'s own `ldims != rdims` guard
/// correctly declines to fire, in contrast with the fixture above where the
/// same guard correctly does fire. This is what proves the package decides
/// the conflict rather than merely restating reflect's silence.
#[test]
fn quantity_add_same_dimension_is_not_a_conflict_end_to_end() {
    let fixture = quantity_add_same_dimension_is_not_a_conflict();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts = reflect_dimension_conflicts(&report);
    assert!(
        reflect_conflicts.is_empty(),
        "reflect.rs must record zero Dimension conflicts when both operands share \
         the same ground dimension; got {reflect_conflicts:?}"
    );

    let export = typefacts::export(&report);
    let mut txn =
        Transaction::new(b"selfhost-parity-quantity-add-same-dimension-is-not-a-conflict".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    // An empty extent may be absent from `settled.extents` entirely or
    // present with length 0 — handle both, matching the Probability/F64
    // bridge test's pattern.
    let dimension_len = settled
        .extents
        .get("DimensionConflict")
        .map_or(0, |extent| extent.len());
    assert_eq!(
        dimension_len, 0,
        "native package must derive zero DimensionConflict rows when both operands \
         share the same ground dimension — DimOp/TyDims rows exist for this query, \
         but DimSameMismatch's own ldims != rdims guard correctly declines to fire"
    );
}

/// Native Dimension slice, flagship cross-spelling bonus: `flagship_pricing_mutation`'s
/// one-character `rate * length` -> `rate / length` mutation breaks the
/// flagship pricing computation's dimensions. The div itself is a mul/div
/// dimension-vector combination (`solve::dimension_binary_step`, deferred —
/// no `DimSameOp` is emitted for it), but its GROUND result
/// (`Dimensioned(Money<EUR>/Kilometre^2)`) is then added to `surcharge`
/// (`Money<EUR>`) — a same-dimension `add` over two ground, unequal operands,
/// which IS this slice's scope. Proves the slice reproduces reflect's
/// Dimension conflict even when the conflicting operand's own type was
/// itself computed by the (deferred) mul/div machinery, not written directly
/// in source.
#[test]
fn flagship_pricing_mutation_derives_one_dimension_conflict() {
    let fixture = flagship_pricing_mutation();
    let report = analyze(&fixture.source, &fixture.resolver);

    let reflect_conflicts = reflect_dimension_conflicts(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one Dimension conflict for the \
         flagship_pricing_mutation fixture; got {reflect_conflicts:?}"
    );
    assert!(
        matches!(&reflect_conflicts[0].kind, ConflictKind::Dimension { op, .. } if op == "add"),
        "the flagship mutation's Dimension conflict must be on the outer \"add\" \
         (rate/length + surcharge), not the deferred div; got {:?}",
        reflect_conflicts[0].kind
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-flagship-pricing-mutation".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let dimension_extent = settled
        .extents
        .get("DimensionConflict")
        .expect("brix.type package must declare a DimensionConflict relation");
    assert_eq!(
        dimension_extent.len(),
        1,
        "native package must derive exactly one DimensionConflict row"
    );

    let native_conflicts = native_dimensions(&export.tokens, dimension_extent);

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived DimensionConflict set must be canonical-byte-identical \
         to reflect.rs's own conflict set, even for the div-computed operand type"
    );
}

/// #15 native rule-side-conditions slice: `rule_impure_effect_row`'s `Loud`
/// rule calls a `console`-effect fn — Appendix E `pure(B, H)` is violated.
/// Reproduced by RESTATEMENT: reflect.rs emits `Fact::RuleImpure` alongside
/// its `ConflictKind::ImpureRule`, and the package re-derives the conflict
/// via `ImpureRuleInRoot`'s `RuleImpureFinding ⋈ RootScope` join.
#[test]
fn rule_impure_effect_row_derives_one_impure_rule_conflict() {
    let fixture = rule_impure_effect_row();
    let report = analyze_rule_fixture(&fixture);

    let reflect_conflicts = reflect_impure_rule_conflicts(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one ImpureRule conflict for the \
         rule_impure_effect_row fixture; got {reflect_conflicts:?}"
    );
    assert_eq!(
        reflect_conflicts[0].kind,
        ConflictKind::ImpureRule,
        "reflect's conflict must be the unit ImpureRule variant"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-rule-impure-effect-row".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let extent = settled
        .extents
        .get("ImpureRuleConflict")
        .expect("brix.type package must declare an ImpureRuleConflict relation");
    assert_eq!(
        extent.len(),
        1,
        "native package must derive exactly one ImpureRuleConflict row"
    );

    let native_conflicts = native_impure_rule_conflicts(&export.tokens, extent);

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived ImpureRuleConflict set must be canonical-byte-identical \
         to reflect.rs's own conflict set (same subject, same scope)"
    );
}

/// #15 native rule-side-conditions slice: `rule_unbound_head_key`'s `Mint`
/// rule's `keyed by (missing)` head names an ident never bound in the
/// (empty) body — Appendix E `keys(H) ⊆ Bindings` is violated. Reproduced by
/// RESTATEMENT via `UnboundHeadKeyInRoot`.
#[test]
fn rule_unbound_head_key_derives_one_unbound_head_key_conflict() {
    let fixture = rule_unbound_head_key();
    let report = analyze_rule_fixture(&fixture);

    let reflect_conflicts = reflect_unbound_head_key_conflicts(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one UnboundHeadKey conflict for the \
         rule_unbound_head_key fixture; got {reflect_conflicts:?}"
    );
    assert_eq!(
        reflect_conflicts[0].kind,
        ConflictKind::UnboundHeadKey {
            key: Ident::new("missing"),
        },
        "reflect's conflict must name the unbound head key `missing`"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-rule-unbound-head-key".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let extent = settled
        .extents
        .get("UnboundHeadKeyConflict")
        .expect("brix.type package must declare an UnboundHeadKeyConflict relation");
    assert_eq!(
        extent.len(),
        1,
        "native package must derive exactly one UnboundHeadKeyConflict row"
    );

    let native_conflicts = native_unbound_head_key_conflicts(&export.tokens, extent);

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived UnboundHeadKeyConflict set must be canonical-byte-identical \
         to reflect.rs's own conflict set (same subject, same key, same scope)"
    );
}

/// #15 native rule-side-conditions slice: `rule_mask_ref_not_edge_bound`'s
/// `Override` rule's `mask(price) by manual` head refers to `price`/`manual`,
/// neither of which is an edge-bound alias in the (empty) body — Appendix
/// E's mask-head side condition is violated for BOTH idents. Reproduced by
/// RESTATEMENT via `MaskRefNotEdgeBoundInRoot`.
#[test]
fn rule_mask_ref_not_edge_bound_derives_two_mask_ref_conflicts() {
    let fixture = rule_mask_ref_not_edge_bound();
    let report = analyze_rule_fixture(&fixture);

    let reflect_conflicts = reflect_mask_ref_conflicts(&report);
    assert_eq!(
        reflect_conflicts.len(),
        2,
        "reflect.rs must record exactly two MaskRefNotEdgeBound conflicts \
         (target `price` and reason `manual`) for the \
         rule_mask_ref_not_edge_bound fixture; got {reflect_conflicts:?}"
    );
    let reflect_vars: BTreeSet<Ident> = reflect_conflicts
        .iter()
        .filter_map(|c| match &c.kind {
            ConflictKind::MaskRefNotEdgeBound { var } => Some(var.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        reflect_vars,
        BTreeSet::from([Ident::new("price"), Ident::new("manual")]),
        "reflect's conflicts must name both unbound mask refs, `price` and `manual`"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-rule-mask-ref-not-edge-bound".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let extent = settled
        .extents
        .get("MaskRefNotEdgeBoundConflict")
        .expect("brix.type package must declare a MaskRefNotEdgeBoundConflict relation");
    assert_eq!(
        extent.len(),
        2,
        "native package must derive exactly two MaskRefNotEdgeBoundConflict rows"
    );

    let native_conflicts = native_mask_ref_conflicts(&export.tokens, extent);

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived MaskRefNotEdgeBoundConflict set must be canonical-byte-identical \
         to reflect.rs's own conflict set (same subject, same var, same scope)"
    );
}

/// #15 native rule-side-conditions slice: `rule_ordinary_fn_on_derived_rel`'s
/// `Summary` rule calls the non-`aggregate` `sumUp` fn on a `Comprehension`
/// over `ComputedPrice`, a `derived: true` relation — Appendix E `Ordinary
/// fn` is violated. Reproduced by RESTATEMENT via
/// `OrdinaryFnOnDerivedRelInRoot`, proving the `QualIdent` verbatim
/// round-trip (`relation` surfaces in the conflict) holds end-to-end.
#[test]
fn rule_ordinary_fn_on_derived_rel_derives_one_ordinary_fn_conflict() {
    let fixture = rule_ordinary_fn_on_derived_rel();
    let report = analyze_rule_fixture(&fixture);

    let reflect_conflicts = reflect_ordinary_fn_conflicts(&report);
    assert_eq!(
        reflect_conflicts.len(),
        1,
        "reflect.rs must record exactly one OrdinaryFnOnDerivedRel conflict for the \
         rule_ordinary_fn_on_derived_rel fixture; got {reflect_conflicts:?}"
    );
    assert_eq!(
        reflect_conflicts[0].kind,
        ConflictKind::OrdinaryFnOnDerivedRel {
            relation: QualIdent::from("ComputedPrice"),
        },
        "reflect's conflict must name the derived relation `ComputedPrice`"
    );

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"selfhost-parity-rule-ordinary-fn-on-derived-rel".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let extent = settled
        .extents
        .get("OrdinaryFnOnDerivedRelConflict")
        .expect("brix.type package must declare an OrdinaryFnOnDerivedRelConflict relation");
    assert_eq!(
        extent.len(),
        1,
        "native package must derive exactly one OrdinaryFnOnDerivedRelConflict row"
    );

    let native_conflicts = native_ordinary_fn_conflicts(&export.tokens, extent);

    assert_eq!(
        conflict_byte_set(&native_conflicts),
        conflict_byte_set(reflect_conflicts.iter().copied()),
        "the native-derived OrdinaryFnOnDerivedRelConflict set must be canonical-byte-identical \
         to reflect.rs's own conflict set (same subject, same relation, same scope) — \
         proving the verbatim QualIdent round-trip holds end-to-end"
    );
}
