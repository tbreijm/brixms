//! Differential parity test harness for native type checker (ADR-0009 §5).

use std::collections::BTreeSet;

use brix_conformance::typecorpus::{self, Category};
use brix_ir::core::Query;
use brix_ir::frontend::{FrontendSource, TableResolver};
use brix_ir::ident::Ident;
use brix_ir::pattern::Pattern;
use brix_ir::reflect::{analyze as brix_analyze, ConflictKind};
use brix_ir::types::{IntWidth, Ty};
use soc_regimes::native::{analyze as native_analyze, translate, NConflict};

/// Pin the observed coverage floor (monotonic across ADR-0009 slices).
///
/// N1: `Mismatch` (scalar). N2: `Arity` (call arg-count vs candidate
/// signatures). N3: `UnknownField` (records/rows) and `Occurs` (activated now
/// that container types Option/Rel/Record are translatable). N4: `NonBoolGuard`
/// (When-guards in Rule / Constraint bodies). N5: `Dimension` (unit conflict detection).
/// N6: `TryNonResult` (postfix `?` on non-Result values). N7: `EpistemicErasure`
/// (Estimate/Missing to plain type, or Probability to Bool). N8a: `Mismatch`
/// (edge-clause role-type mismatch in Rule / Constraint bodies).
const COVERAGE_FLOOR: usize = 24;

fn reflect_category(kind: &ConflictKind) -> Option<Category> {
    match kind {
        ConflictKind::Mismatch { .. } => Some(Category::Mismatch),
        ConflictKind::Arity { .. } => Some(Category::Arity),
        ConflictKind::UnknownField { .. } => Some(Category::UnknownField),
        ConflictKind::NonBool { .. } => Some(Category::NonBoolGuard),
        ConflictKind::Occurs { .. } => Some(Category::Occurs),
        ConflictKind::Dimension { .. } => Some(Category::Dimension),
        ConflictKind::TryNonResult { .. } => Some(Category::TryNonResult),
        ConflictKind::EpistemicErasure { .. } => Some(Category::EpistemicErasure),
        ConflictKind::ImpureRule
        | ConflictKind::NondeterministicRule
        | ConflictKind::DivergentRule
        | ConflictKind::UnboundHeadKey { .. }
        | ConflictKind::MaskRefNotEdgeBound { .. }
        | ConflictKind::OrdinaryFnOnDerivedRel { .. } => None,
    }
}

fn conflict_category(c: &NConflict) -> Category {
    match c {
        NConflict::Mismatch { .. } => Category::Mismatch,
        NConflict::Occurs { .. } => Category::Occurs,
        NConflict::Arity { .. } => Category::Arity,
        NConflict::UnknownField { .. } => Category::UnknownField,
        NConflict::NonBool { .. } => Category::NonBoolGuard,
        NConflict::Dimension { .. } => Category::Dimension,
        NConflict::TryNonResult { .. } => Category::TryNonResult,
        NConflict::EpistemicErasure { .. } => Category::EpistemicErasure,
    }
}

#[test]
fn native_type_parity_corpus_coverage() {
    let mut fixtures = typecorpus::all_type_fixtures();
    fixtures.push(typecorpus::plain_scalar_mismatch());
    // Discriminator (selfhost-only, not in all_type_fixtures): an overload whose
    // *non-first* candidate matches the arg count → NO Arity conflict.
    fixtures.push(typecorpus::arity_non_first_candidate_match_is_not_a_conflict());
    fixtures.push(typecorpus::quantity_add_dimension_mismatch());
    fixtures.push(typecorpus::quantity_add_same_dimension_is_not_a_conflict());
    fixtures.push(typecorpus::try_over_result_is_not_a_conflict());
    fixtures.push(typecorpus::probability_f64_bridge_is_not_a_conflict());
    fixtures.push(typecorpus::estimate_vs_container_erasure());
    fixtures.push(typecorpus::cross_epistemic_wrapper_mismatch());
    fixtures.push(typecorpus::estimate_same_ctor_mismatch());
    fixtures.push(typecorpus::same_container_leaf_no_double_count());
    let total = fixtures.len();
    let mut covered = 0;
    // N1: Mismatch. N2: Arity. N3: UnknownField and Occurs. N4: NonBoolGuard. N5: Dimension. N6: TryNonResult. N7: EpistemicErasure.
    let allowed_cats = BTreeSet::from([
        Category::Mismatch,
        Category::Arity,
        Category::UnknownField,
        Category::Occurs,
        Category::NonBoolGuard,
        Category::Dimension,
        Category::TryNonResult,
        Category::EpistemicErasure,
    ]);

    for fixture in &fixtures {
        let brix_report = brix_analyze(&fixture.source, &fixture.resolver);
        let brix_cats: BTreeSet<Category> = brix_report
            .conflicts
            .iter()
            .filter_map(|c| reflect_category(&c.kind))
            .collect();

        let n = translate(&fixture.source, &fixture.resolver);
        if let Some(native_src) = n {
            if brix_cats.is_subset(&allowed_cats) {
                let native_report = native_analyze(&native_src);
                let native_cats: BTreeSet<Category> = native_report
                    .conflicts
                    .iter()
                    .map(conflict_category)
                    .collect();
                assert_eq!(
                    native_cats, brix_cats,
                    "Fixture category mismatch for '{}'",
                    fixture.label
                );
                covered += 1;
            }
        }
    }

    println!(
        "Native type parity coverage: {}/{} fixtures covered",
        covered, total
    );
    assert!(
        covered >= COVERAGE_FLOOR,
        "Coverage regressed below floor: got {}, expected >= {}",
        covered,
        COVERAGE_FLOOR
    );
}

#[test]
fn translatable_well_typed_fixture_has_no_conflicts() {
    let o = typecorpus::Origins::new("WellTypedScalar");
    let source = FrontendSource {
        functions: vec![],
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("WellTypedScalar"),
            params: vec![(Ident::new("x"), Ty::Int(IntWidth::Int))],
            body: Pattern::default(),
            yields: o.var("x"),
            result: Ty::Int(IntWidth::Int),
        }],
    };
    let resolver = TableResolver::new();
    let native_src =
        translate(&source, &resolver).expect("should translate well-typed scalar query");
    let report = native_analyze(&native_src);
    assert!(report.is_consistent(), "Expected no conflicts: {report:#?}");
    assert!(!report.has_types.is_empty(), "Expected non-empty has_types");
}

#[test]
fn unsupported_construct_returns_none() {
    let o = typecorpus::Origins::new("Unsupported");
    let source = FrontendSource {
        functions: vec![],
        rules: vec![],
        constraints: vec![],
        queries: vec![Query {
            name: Ident::new("Unsupported"),
            params: vec![(
                Ident::new("res"),
                Ty::List(Box::new(Ty::Int(IntWidth::Int))),
            )],
            body: Pattern::default(),
            yields: o.var("res"),
            result: Ty::List(Box::new(Ty::Int(IntWidth::Int))),
        }],
    };
    let resolver = TableResolver::new();
    let native_src = translate(&source, &resolver);
    assert!(
        native_src.is_none(),
        "List type construct should return None"
    );
}
