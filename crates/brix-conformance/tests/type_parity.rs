//! Differential parity harness (#15 PR2, promoted into `brix-conformance` by
//! #15 PR6: "conformance: differential fixture corpus as a reusable
//! suite").
//!
//! `infer::infer_source` (the trusted, non-self-hosted bootstrap unification
//! checker) and `reflect::analyze` (the fact-oriented reference analyzer the
//! native `brix.type` package mirrors, see `selfhost_parity.rs`) share one
//! algebra — `brix_ir::solve` — instead of two independent copies that used
//! to silently diverge (Probability↔F64 bridge, dimension-vs-variable
//! solving, row-check symmetry, Option/Result descent, occurs-check depth;
//! see the #15 issue's "Trajectory plan" ruling). This file is the property
//! test that proves the two checkers cannot re-diverge undetected, driven by
//! the fixture corpus in `brix_conformance::typecorpus` (formerly
//! `crates/brix-ir/tests/parity.rs`, deleted by this promotion — `brix-ir`
//! cannot depend on `brix-conformance`, so the corpus and this harness had
//! to move here, not the other way around) — for every corpus fixture it
//! asserts the frozen parity contract:
//!
//! 1. **Verdict equivalence**: `analyze(S,Σ).is_consistent() ⟺
//!    infer_source(S,Σ).is_empty()`.
//! 2. **Category-set equivalence**: every `TypeError`/`TypeConflict` maps to
//!    one of the [`typecorpus::Category`] variants, compared as a canonical
//!    *set* — never a sequence — against both checkers' own output *and*
//!    the fixture's own [`typecorpus::TypeFixture::expected_categories`].
//!    `infer` cascades in body-traversal order and `reflect`'s facts are
//!    derivation-set-valued, so a sequence comparison would fail on harmless
//!    reordering rather than a real divergence.
//! 3. **Type mirror**: every zonked `Expr.ty` `infer_source` leaves behind
//!    has a matching `Fact::HasType{Subject::Expr{origin}, ty}` in
//!    `analyze`'s report, with the identical resolved type.
//!
//! # #15 PR4: a second, independent parity axis
//!
//! Appendix E's *rule side conditions* (`pure`/`det`/`nondiverge`,
//! `keys(H) ⊆ Bindings`, mask-head, `Ordinary fn`) are not type-inference
//! judgments — `infer_source` has no notion of them at all, only
//! `brix_ir::check::check_rule` (the trusted checker) and
//! `brix_ir::reflect::analyze` (mirrored onto `ConflictKind`, #15 PR4) do.
//! So these get their own `RuleCategory`/`assert_rule_side_condition_parity`
//! axis below, entirely separate from `Category`/`assert_parity`.

use std::collections::{BTreeMap, BTreeSet};

use brix_ir::check::{check_rule, Finding};
use brix_ir::core::{Expr, ExprKind, ExprOrigin};
use brix_ir::infer::{infer_source, TypeError};
use brix_ir::pattern::{Clause, Pattern};
use brix_ir::reflect::{analyze, ConflictKind, Fact, ReflectiveReport, Subject};
use brix_ir::types::Ty;

use brix_conformance::typecorpus::{self, Category, RuleCategory, RuleFixture, TypeFixture};

/// `TypeError` -> `Category`.
fn infer_category(error: &TypeError) -> Category {
    match error {
        TypeError::Mismatch { .. } => Category::Mismatch,
        TypeError::Dimension { .. } => Category::Dimension,
        TypeError::Arity { .. } => Category::Arity,
        TypeError::UnknownField { .. } => Category::UnknownField,
        TypeError::NonBoolGuard { .. } => Category::NonBoolGuard,
        TypeError::TryNonResult { .. } => Category::TryNonResult,
        TypeError::Occurs { .. } => Category::Occurs,
        TypeError::EpistemicErasure { .. } => Category::EpistemicErasure,
        // Overload resolution is a dedicated infer diagnostic; reflect reports
        // the same failure as `ConflictKind::Mismatch`.
        TypeError::NoMatchingOverload { .. } | TypeError::AmbiguousOverload { .. } => {
            Category::Mismatch
        }
    }
}

/// `reflect::ConflictKind` -> `Category`, per the #15 PR3 rewiring: the
/// harness maps from the frozen `ConflictKind` enum instead of a free-text
/// `operation` string. The map is 1:1 and exhaustive over the eight
/// *type-inference* variants. Returns `None` for the #15 PR4 Appendix E rule
/// side-condition variants (`ImpureRule`..`OrdinaryFnOnDerivedRel`): those
/// have no `infer.rs` counterpart at all, so this type-inference-parity axis
/// must not count them; they get their own `RuleCategory`/
/// `assert_rule_side_condition_parity` axis below.
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

fn collect_expr_types(pattern: &Pattern, out: &mut BTreeMap<ExprOrigin, Ty>) {
    for clause in &pattern.clauses {
        match clause {
            Clause::Let { expr, .. } | Clause::When(expr) => collect_expr_type(expr, out),
            Clause::Any(cases) => {
                for case in cases {
                    collect_expr_types(case, out);
                }
            }
            Clause::Exists(p) | Clause::Without(p) | Clause::Optional(p) | Clause::Cross(p) => {
                collect_expr_types(p, out)
            }
            _ => {}
        }
    }
}

/// Mirrors `Infer::zonk_expr`'s traversal exactly, so it visits the same
/// node set `Reflect::expr` records `HasType` facts for.
fn collect_expr_type(expr: &Expr, out: &mut BTreeMap<ExprOrigin, Ty>) {
    out.insert(expr.origin, expr.ty.clone());
    match &*expr.kind {
        ExprKind::Call { args, .. } => {
            for a in args {
                collect_expr_type(a, out);
            }
        }
        ExprKind::Field { base, .. } => collect_expr_type(base, out),
        ExprKind::Record { fields } => {
            for (_, v) in fields {
                collect_expr_type(v, out);
            }
        }
        ExprKind::If { cond, then, els } => {
            collect_expr_type(cond, out);
            collect_expr_type(then, out);
            collect_expr_type(els, out);
        }
        ExprKind::Try { inner, .. } => collect_expr_type(inner, out),
        ExprKind::Comprehension { pattern, yields } => {
            collect_expr_types(pattern, out);
            if let Some(y) = yields {
                collect_expr_type(y, out);
            }
        }
        ExprKind::Let { value, body, .. } => {
            collect_expr_type(value, out);
            collect_expr_type(body, out);
        }
        ExprKind::Var(_) | ExprKind::Lit(_) => {}
    }
}

/// Run both checkers over one corpus [`TypeFixture`] and assert the full
/// parity contract, including the fixture's own corpus-declared
/// `expected_categories`. Returns the reflective report and infer errors so
/// individual `#[test]`s can layer additional fixture-specific assertions
/// on top (mirroring what the pre-promotion `crates/brix-ir/tests/
/// parity.rs` did inline).
fn assert_parity(fixture: &TypeFixture) -> (ReflectiveReport, Vec<TypeError>) {
    let TypeFixture {
        label,
        source,
        resolver,
        expected_categories,
        ..
    } = fixture;

    let report = analyze(source, resolver);
    let mut bootstrap = source.clone();
    let errors = infer_source(&mut bootstrap, resolver);

    // 1. Verdict equivalence.
    assert_eq!(
        report.is_consistent(),
        errors.is_empty(),
        "{label}: verdict mismatch — reflect.is_consistent()={}, infer errors={errors:?}",
        report.is_consistent(),
    );

    // 2. Category-set equivalence (canonical sets, not sequences) — infer
    // vs reflect, and both against the corpus's own declared expectation.
    let infer_categories: BTreeSet<Category> = errors.iter().map(infer_category).collect();
    let reflect_categories: BTreeSet<Category> = report
        .conflicts
        .iter()
        .filter_map(|c| reflect_category(&c.kind))
        .collect();
    assert_eq!(
        infer_categories, reflect_categories,
        "{label}: category-set mismatch\ninfer errors: {errors:#?}\nreflect conflicts: {:#?}",
        report.conflicts
    );
    assert_eq!(
        &infer_categories, expected_categories,
        "{label}: category-set mismatch vs the corpus's declared expected_categories"
    );

    // 3. Type mirror: every zonked `Expr.ty` from `infer` has a matching
    // `Fact::HasType{Subject::Expr{origin}, ty}` in `analyze`, with the
    // equal resolved type.
    let mut infer_types = BTreeMap::new();
    for rule in &bootstrap.rules {
        collect_expr_types(&rule.body, &mut infer_types);
    }
    for constraint in &bootstrap.constraints {
        collect_expr_types(&constraint.body, &mut infer_types);
    }
    for query in &bootstrap.queries {
        collect_expr_types(&query.body, &mut infer_types);
        collect_expr_type(&query.yields, &mut infer_types);
    }

    let mut reflect_types = BTreeMap::new();
    for derivation in &report.facts {
        if let Fact::HasType {
            subject: Subject::Expr { origin },
            ty,
            ..
        } = &derivation.fact
        {
            reflect_types.insert(*origin, ty.clone());
        }
    }

    for (origin, ty) in &infer_types {
        let reflected = reflect_types.get(origin).unwrap_or_else(|| {
            panic!("{label}: infer zonked {origin:?} to {ty} but reflect recorded no HasType fact for it")
        });
        assert_eq!(
            ty, reflected,
            "{label}: type-mirror mismatch at {origin:?}: infer={ty}, reflect={reflected}"
        );
    }

    (report, errors)
}

/// Run both `check_rule` (trusted) and `reflect::analyze` (reflective) over
/// one corpus [`RuleFixture`] and assert they agree: at least one Appendix E
/// finding fires, the two checkers' category *sets* are identical, and both
/// equal the fixture's own corpus-declared `expected_categories`.
fn assert_rule_side_condition_parity(fixture: &RuleFixture) {
    let RuleFixture {
        label,
        rule,
        resolver,
        expected_categories,
        ..
    } = fixture;

    let findings = check_rule(rule, resolver);
    let source = brix_ir::frontend::FrontendSource {
        functions: Vec::new(),
        rules: vec![rule.clone()],
        constraints: vec![],
        queries: vec![],
    };
    let report = analyze(&source, resolver);

    let finding_categories: BTreeSet<RuleCategory> =
        findings.iter().filter_map(finding_category).collect();
    let conflict_categories: BTreeSet<RuleCategory> = report
        .conflicts
        .iter()
        .filter_map(|c| conflict_rule_category(&c.kind))
        .collect();

    assert!(
        !finding_categories.is_empty(),
        "{label}: expected at least one Appendix E finding, got none: {findings:#?}"
    );
    assert_eq!(
        finding_categories, conflict_categories,
        "{label}: category-set mismatch\nfindings: {findings:#?}\nconflicts: {:#?}",
        report.conflicts
    );
    assert_eq!(
        &finding_categories, expected_categories,
        "{label}: category-set mismatch vs the corpus's declared expected_categories"
    );
}

/// `check::Finding` -> `RuleCategory`. `None` for findings outside this
/// axis (`NonCanonicalKey`/`AbsenceWithoutWitness`/`UnknownRelation`
/// predate #15 PR4 and have no `ConflictKind` mirror to compare against).
fn finding_category(finding: &Finding) -> Option<RuleCategory> {
    match finding {
        Finding::ImpureRule { .. } => Some(RuleCategory::Impure),
        Finding::NondeterministicRule { .. } => Some(RuleCategory::Nondeterministic),
        Finding::DivergentRule { .. } => Some(RuleCategory::Divergent),
        Finding::UnboundHeadKey { .. } => Some(RuleCategory::UnboundHeadKey),
        Finding::MaskRefNotEdgeBound { .. } => Some(RuleCategory::MaskRefNotEdgeBound),
        Finding::OrdinaryFnOnDerivedRel { .. } => Some(RuleCategory::OrdinaryFnOnDerivedRel),
        Finding::NonCanonicalKey { .. }
        | Finding::AbsenceWithoutWitness { .. }
        | Finding::UnknownRelation { .. }
        // Function-body checks (issue #47) are outside the rule-category axis
        // and have no reflective `ConflictKind` mirror.
        | Finding::UndeclaredFnEffect { .. }
        | Finding::TotalFnFallible { .. } => None,
    }
}

/// `reflect::ConflictKind` -> `RuleCategory`, the mirror-side counterpart of
/// `finding_category`. `None` for the eight type-inference variants (those
/// belong to `reflect_category`/`Category` instead).
fn conflict_rule_category(kind: &ConflictKind) -> Option<RuleCategory> {
    match kind {
        ConflictKind::ImpureRule => Some(RuleCategory::Impure),
        ConflictKind::NondeterministicRule => Some(RuleCategory::Nondeterministic),
        ConflictKind::DivergentRule => Some(RuleCategory::Divergent),
        ConflictKind::UnboundHeadKey { .. } => Some(RuleCategory::UnboundHeadKey),
        ConflictKind::MaskRefNotEdgeBound { .. } => Some(RuleCategory::MaskRefNotEdgeBound),
        ConflictKind::OrdinaryFnOnDerivedRel { .. } => Some(RuleCategory::OrdinaryFnOnDerivedRel),
        ConflictKind::Mismatch { .. }
        | ConflictKind::Arity { .. }
        | ConflictKind::UnknownField { .. }
        | ConflictKind::NonBool { .. }
        | ConflictKind::Occurs { .. }
        | ConflictKind::Dimension { .. }
        | ConflictKind::TryNonResult { .. }
        | ConflictKind::EpistemicErasure { .. } => None,
    }
}

#[test]
fn flagship_pricing_mutation_agrees() {
    assert_parity(&typecorpus::flagship_pricing_mutation());
}

#[test]
fn non_bool_guard_agrees() {
    assert_parity(&typecorpus::non_bool_guard());
}

#[test]
fn arity_mismatch_agrees() {
    assert_parity(&typecorpus::arity_mismatch());
}

#[test]
fn role_mismatch_agrees() {
    assert_parity(&typecorpus::role_mismatch());
}

#[test]
fn field_failure_agrees() {
    assert_parity(&typecorpus::field_failure());
}

#[test]
fn occurs_check_agrees() {
    assert_parity(&typecorpus::occurs_check());
}

#[test]
fn closed_row_extra_field_is_a_mismatch() {
    assert_parity(&typecorpus::closed_row_extra_field());
}

#[test]
fn open_row_extra_field_is_admitted() {
    assert_parity(&typecorpus::open_row_extra_field());
}

#[test]
fn constraint_non_bool_guard_agrees() {
    assert_parity(&typecorpus::constraint_non_bool_guard());
}

#[test]
fn constraint_role_mismatch_agrees() {
    assert_parity(&typecorpus::constraint_role_mismatch());
}

#[test]
fn try_non_result_agrees() {
    assert_parity(&typecorpus::try_non_result());
}

#[test]
fn rule_impure_effect_row_agrees() {
    assert_rule_side_condition_parity(&typecorpus::rule_impure_effect_row());
}

#[test]
fn rule_unbound_head_key_agrees() {
    assert_rule_side_condition_parity(&typecorpus::rule_unbound_head_key());
}

#[test]
fn rule_mask_ref_not_edge_bound_agrees() {
    assert_rule_side_condition_parity(&typecorpus::rule_mask_ref_not_edge_bound());
}

#[test]
fn rule_ordinary_fn_on_derived_rel_agrees() {
    assert_rule_side_condition_parity(&typecorpus::rule_ordinary_fn_on_derived_rel());
}

#[test]
fn estimate_to_plain_erasure_agrees() {
    let fixture = typecorpus::estimate_to_plain_erasure();
    let (report, _) = assert_parity(&fixture);
    assert!(
        report
            .conflicts
            .iter()
            .any(|c| matches!(c.kind, ConflictKind::EpistemicErasure { .. })),
        "expected an EpistemicErasure conflict: {report:#?}"
    );
}

#[test]
fn probability_to_bool_erasure_agrees() {
    let fixture = typecorpus::probability_to_bool_erasure();
    let (report, _) = assert_parity(&fixture);
    assert!(
        report
            .conflicts
            .iter()
            .any(|c| matches!(c.kind, ConflictKind::EpistemicErasure { .. })),
        "expected an EpistemicErasure conflict: {report:#?}"
    );
}

#[test]
fn missing_to_plain_implicit_coercion_is_an_erasure() {
    let fixture = typecorpus::missing_to_plain_implicit_coercion();
    let (report, _) = assert_parity(&fixture);
    assert!(
        report
            .conflicts
            .iter()
            .any(|c| matches!(c.kind, ConflictKind::EpistemicErasure { .. })),
        "expected an EpistemicErasure conflict: {report:#?}"
    );
}

#[test]
fn missing_well_typed_flow_agrees() {
    let fixture = typecorpus::missing_well_typed_flow();
    let (report, errors) = assert_parity(&fixture);
    assert!(report.is_consistent(), "{report:#?}");
    assert!(errors.is_empty(), "{errors:?}");
}
