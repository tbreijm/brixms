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

/// Pin the observed coverage floor for slice N1.
///
/// N1 covers `Mismatch` differentially (scalar fragment). `Occurs` is NOT
/// covered here: every real corpus `Occurs` fixture forces occurs *into a
/// container* (`Option`/`Rel`), which the N1 scalar fragment cannot translate;
/// a scalar-only occurs case is not exercised by the language. Native occurs
/// detection is unit-tested in `soc_regimes::native::analyze`; its differential
/// coverage arrives with the container slice. Do NOT fabricate a scalar occurs
/// fixture (it would require patching the brix-ir oracle).
const COVERAGE_FLOOR: usize = 1;

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
    }
}

#[test]
fn native_type_parity_corpus_coverage() {
    let mut fixtures = typecorpus::all_type_fixtures();
    fixtures.push(typecorpus::plain_scalar_mismatch());
    let total = fixtures.len();
    let mut covered = 0;
    // N1 covers Mismatch only (see COVERAGE_FLOOR). Occurs deferred to the container slice.
    let allowed_cats = BTreeSet::from([Category::Mismatch]);

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
                Ident::new("opt"),
                Ty::Option(Box::new(Ty::Int(IntWidth::Int))),
            )],
            body: Pattern::default(),
            yields: o.var("opt"),
            result: Ty::Option(Box::new(Ty::Int(IntWidth::Int))),
        }],
    };
    let resolver = TableResolver::new();
    let native_src = translate(&source, &resolver);
    assert!(
        native_src.is_none(),
        "Option type construct should return None"
    );
}
