//! Differential parity, `FactId`-for-`FactId` shadow parity, and
//! `ScopedWorldNonLeak` gates for `crates/soc-regimes`'s [`StructuralRegime`]
//! (`Build_Plan_v3_SOC.md` Step 5(b); ADR-0002 §6/§6.1/§7).
//!
//! Three non-vacuous gates:
//!
//! (a) **14/14 `ConflictKind` differential parity.** For every corpus fixture
//!     (`typecorpus::all_type_fixtures` covering the 8 `Category` kinds,
//!     `typecorpus::all_rule_fixtures` covering the 6 `RuleCategory` kinds —
//!     together all 14 `ConflictKind` variants), the structural regime
//!     projects `reflect::analyze`'s report into its own
//!     [`soc_regimes::ProjectedConflict`]s, and this file's OWN
//!     `category_of`/`rule_category_of` — written independently of, and not
//!     calling, `type_parity.rs`'s private `reflect_category`/
//!     `conflict_rule_category` (a separate integration-test binary; those
//!     functions are not even visible here) — classifies them. The resulting
//!     category *set* must agree with BOTH `reflect::analyze`'s own
//!     conflicts and `infer::infer_source`'s errors, and with the fixture's
//!     own corpus-declared `expected_categories`.
//! (b) **`FactId`-for-`FactId` shadow parity.** For every `HasType` fact in
//!     the corpus, an independently-rebuilt `Fact::HasType` — built from
//!     nothing but the `(subject, ty)` the structural projection retained —
//!     re-derives `FactId::derive` and must equal `reflect`'s own
//!     `Derivation.id`, byte-for-byte. Non-vacuous: the test never reads
//!     `derivation.id` to build the reconstruction, only to check it.
//! (c) **`ScopedWorldNonLeak` (green).** Using the additive
//!     `ContextId::extend`, a `HasType` judgement derived under a child
//!     (assumption) context is asserted to be absent from the parent
//!     (root) context's projected judgement set, even though both share the
//!     identical underlying `Realizes` proposition.

use std::collections::BTreeSet;

use brix_ir::frontend::FrontendSource;
use brix_ir::infer::{infer_source, TypeError};
use brix_ir::reflect::{analyze, ConflictKind, Fact, FactId, ScopeId};

use brix_conformance::typecorpus::{
    self, Category, ConformanceCategory, RuleCategory, RuleFixture, TypeFixture,
};
use brix_semantic::ContextId;
use soc_regimes::StructuralRegime;

// --- independent category mappings (module doc: not shared with type_parity.rs) ---

/// `TypeError` -> `Category`, written fresh for this gate.
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
        TypeError::NoMatchingOverload { .. } | TypeError::AmbiguousOverload { .. } => {
            Category::Mismatch
        }
    }
}

/// `ConflictKind` -> `Category`, over the structural regime's OWN
/// [`soc_regimes::ProjectedConflict::kind`] — a fresh match, independent of
/// `type_parity.rs`'s private `reflect_category`.
fn category_of(kind: &ConflictKind) -> Option<Category> {
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

/// `ConflictKind` -> `RuleCategory`, the Appendix-E-axis counterpart of
/// [`category_of`], likewise independent of `type_parity.rs`.
fn rule_category_of(kind: &ConflictKind) -> Option<RuleCategory> {
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

// --- gate (a): 14/14 differential parity ---

/// Drive one [`TypeFixture`] through `analyze`, `infer_source`, and the
/// structural regime's own conflict projection; assert three-way category-set
/// agreement plus verdict equivalence.
fn assert_structural_parity_type(fixture: &TypeFixture) {
    let report = analyze(&fixture.source, &fixture.resolver);
    let mut bootstrap = fixture.source.clone();
    let infer_errors = infer_source(&mut bootstrap, &fixture.resolver);

    let regime = StructuralRegime::new();
    let projected = regime.project_conflicts(&report, ContextId::root());

    let structural_categories: BTreeSet<Category> = projected
        .iter()
        .filter_map(|p| category_of(&p.kind))
        .collect();
    let infer_categories: BTreeSet<Category> = infer_errors.iter().map(infer_category).collect();

    // Verdict equivalence, three ways: analyze, infer, and the structural
    // regime's own (independently projected) conflict set.
    assert_eq!(
        report.is_consistent(),
        infer_errors.is_empty(),
        "{}: reflect/infer verdict mismatch",
        fixture.label
    );
    assert_eq!(
        projected.is_empty(),
        report.is_consistent(),
        "{}: structural regime's projected-conflict emptiness disagrees with reflect's verdict",
        fixture.label
    );

    // Category-set equivalence: structural (own mapping) vs infer, and both
    // vs the corpus's own declared expectation.
    assert_eq!(
        structural_categories, infer_categories,
        "{}: category-set mismatch (structural vs infer)\nprojected: {:#?}\ninfer errors: {:#?}",
        fixture.label, projected, infer_errors
    );
    assert_eq!(
        &structural_categories, &fixture.expected_categories,
        "{}: category-set mismatch (structural vs corpus-declared expected_categories)",
        fixture.label
    );

    // Every projected conflict judgement must be Outcome::Unknown, never a
    // regime-published Derived/Proven/Refuted (ADR-0002 §4.1/§7).
    for p in &projected {
        assert_eq!(
            p.judgement.outcome,
            brix_semantic::Outcome::Unknown,
            "{}: a conflict must project to Unknown, never a stronger outcome",
            fixture.label
        );
    }
}

/// The [`RuleFixture`] counterpart of [`assert_structural_parity_type`].
fn assert_structural_parity_rule(fixture: &RuleFixture) {
    let source = FrontendSource {
        functions: Vec::new(),
        rules: vec![fixture.rule.clone()],
        constraints: vec![],
        queries: vec![],
    };
    let report = analyze(&source, &fixture.resolver);

    let regime = StructuralRegime::new();
    let projected = regime.project_conflicts(&report, ContextId::root());

    let structural_categories: BTreeSet<RuleCategory> = projected
        .iter()
        .filter_map(|p| rule_category_of(&p.kind))
        .collect();

    assert!(
        !structural_categories.is_empty(),
        "{}: expected at least one Appendix E rule-category conflict, got none: {projected:#?}",
        fixture.label
    );
    assert_eq!(
        &structural_categories, &fixture.expected_categories,
        "{}: category-set mismatch (structural vs corpus-declared expected_categories)",
        fixture.label
    );
    for p in &projected {
        assert_eq!(
            p.judgement.outcome,
            brix_semantic::Outcome::Unknown,
            "{}: a conflict must project to Unknown, never a stronger outcome",
            fixture.label
        );
    }
}

macro_rules! type_parity_test {
    ($test_name:ident, $fixture_fn:path) => {
        #[test]
        fn $test_name() {
            assert_structural_parity_type(&$fixture_fn());
        }
    };
}

macro_rules! rule_parity_test {
    ($test_name:ident, $fixture_fn:path) => {
        #[test]
        fn $test_name() {
            assert_structural_parity_rule(&$fixture_fn());
        }
    };
}

// Category axis (8 kinds): Mismatch, Dimension, Arity, UnknownField,
// TryNonResult, NonBoolGuard, Occurs, EpistemicErasure. Every one is covered
// by at least one of the fixtures below (see each fixture's own doc comment
// in typecorpus.rs for which `Category` it targets).
type_parity_test!(
    structural_flagship_pricing_mutation_is_dimension,
    typecorpus::flagship_pricing_mutation
);
type_parity_test!(
    structural_non_bool_guard_is_non_bool_guard,
    typecorpus::non_bool_guard
);
type_parity_test!(
    structural_arity_mismatch_is_arity,
    typecorpus::arity_mismatch
);
type_parity_test!(
    structural_role_mismatch_is_mismatch,
    typecorpus::role_mismatch
);
type_parity_test!(
    structural_field_failure_is_unknown_field,
    typecorpus::field_failure
);
type_parity_test!(structural_occurs_check_is_occurs, typecorpus::occurs_check);
type_parity_test!(
    structural_closed_row_extra_field_is_unknown_field,
    typecorpus::closed_row_extra_field
);
type_parity_test!(
    structural_open_row_extra_field_is_consistent,
    typecorpus::open_row_extra_field
);
type_parity_test!(
    structural_constraint_non_bool_guard_is_non_bool_guard,
    typecorpus::constraint_non_bool_guard
);
type_parity_test!(
    structural_constraint_role_mismatch_is_mismatch,
    typecorpus::constraint_role_mismatch
);
type_parity_test!(
    structural_try_non_result_is_try_non_result,
    typecorpus::try_non_result
);
type_parity_test!(
    structural_estimate_to_plain_erasure_is_epistemic_erasure,
    typecorpus::estimate_to_plain_erasure
);
type_parity_test!(
    structural_probability_to_bool_erasure_is_epistemic_erasure,
    typecorpus::probability_to_bool_erasure
);
type_parity_test!(
    structural_missing_to_plain_implicit_coercion_is_epistemic_erasure,
    typecorpus::missing_to_plain_implicit_coercion
);
type_parity_test!(
    structural_missing_well_typed_flow_is_consistent,
    typecorpus::missing_well_typed_flow
);

// Rule-side-condition axis (6 kinds): Impure, Nondeterministic, Divergent,
// UnboundHeadKey, MaskRefNotEdgeBound, OrdinaryFnOnDerivedRel. Every one is
// covered by exactly one fixture below — together with the 8 `Category`
// fixtures above, all 14 `ConflictKind` variants are exercised, none
// `#[ignore]`d.
rule_parity_test!(
    structural_rule_impure_effect_row_is_impure,
    typecorpus::rule_impure_effect_row
);
rule_parity_test!(
    structural_rule_nondeterministic_effect_row_is_impure_and_nondeterministic,
    typecorpus::rule_nondeterministic_effect_row
);
rule_parity_test!(
    structural_rule_divergent_call_is_divergent,
    typecorpus::rule_divergent_call
);
rule_parity_test!(
    structural_rule_unbound_head_key_is_unbound_head_key,
    typecorpus::rule_unbound_head_key
);
rule_parity_test!(
    structural_rule_mask_ref_not_edge_bound_is_mask_ref_not_edge_bound,
    typecorpus::rule_mask_ref_not_edge_bound
);
rule_parity_test!(
    structural_rule_ordinary_fn_on_derived_rel_is_ordinary_fn_on_derived_rel,
    typecorpus::rule_ordinary_fn_on_derived_rel
);

/// A single roll-up test asserting the headline coverage number itself: all
/// 8 `Category` + 6 `RuleCategory` = 14 `ConflictKind` variants are exercised
/// by at least one corpus fixture above, non-`#[ignore]`d.
#[test]
fn fourteen_of_fourteen_conflict_kinds_are_covered_non_ignored() {
    let mut covered_categories = BTreeSet::new();
    for fixture in typecorpus::all_type_fixtures() {
        let report = analyze(&fixture.source, &fixture.resolver);
        let regime = StructuralRegime::new();
        let projected = regime.project_conflicts(&report, ContextId::root());
        for p in &projected {
            if let Some(c) = category_of(&p.kind) {
                covered_categories.insert(c);
            }
        }
    }
    let all_categories: BTreeSet<Category> = BTreeSet::from([
        Category::Mismatch,
        Category::Dimension,
        Category::Arity,
        Category::UnknownField,
        Category::TryNonResult,
        Category::NonBoolGuard,
        Category::Occurs,
        Category::EpistemicErasure,
    ]);
    assert_eq!(
        covered_categories, all_categories,
        "expected all 8 Category kinds covered by the type-fixture corpus"
    );

    let mut covered_rule_categories = BTreeSet::new();
    for fixture in typecorpus::all_rule_fixtures() {
        let source = FrontendSource {
            functions: Vec::new(),
            rules: vec![fixture.rule.clone()],
            constraints: vec![],
            queries: vec![],
        };
        let report = analyze(&source, &fixture.resolver);
        let regime = StructuralRegime::new();
        let projected = regime.project_conflicts(&report, ContextId::root());
        for p in &projected {
            if let Some(c) = rule_category_of(&p.kind) {
                covered_rule_categories.insert(c);
            }
        }
    }
    let all_rule_categories: BTreeSet<RuleCategory> = BTreeSet::from([
        RuleCategory::Impure,
        RuleCategory::Nondeterministic,
        RuleCategory::Divergent,
        RuleCategory::UnboundHeadKey,
        RuleCategory::MaskRefNotEdgeBound,
        RuleCategory::OrdinaryFnOnDerivedRel,
    ]);
    assert_eq!(
        covered_rule_categories, all_rule_categories,
        "expected all 6 RuleCategory kinds covered by the rule-fixture corpus"
    );

    // 8 + 6 = 14: every reflect::ConflictKind variant.
    assert_eq!(covered_categories.len() + covered_rule_categories.len(), 14);
}

// --- gate (b): FactId-for-FactId shadow parity ---

/// For every `HasType` fact in `fixture`, reconstruct `FactId::derive`
/// independently from the structural projection's retained `(subject, ty)`
/// and assert it equals `reflect`'s own `Derivation.id`. Non-vacuous: the
/// reconstruction never reads `derivation.id`.
fn assert_has_type_shadow_parity(fixture: &TypeFixture) -> usize {
    let report = analyze(&fixture.source, &fixture.resolver);
    let regime = StructuralRegime::new();
    let projection = regime.project(&report, ContextId::root());

    let mut checked = 0;
    for derivation in &report.facts {
        let Fact::HasType { subject, ty, scope } = &derivation.fact else {
            continue;
        };
        // Every corpus fixture is root-scoped (reflect has no scope
        // machinery of its own yet) — the anchor this gate depends on.
        assert_eq!(
            *scope,
            ScopeId::root(),
            "{}: corpus HasType facts are expected to be root-scoped",
            fixture.label
        );

        let retained = projection
            .has_type
            .iter()
            .find(|p| &p.subject == subject && &p.ty == ty)
            .unwrap_or_else(|| {
                panic!(
                    "{}: HasType fact for {subject:?}:{ty} missing from the structural projection",
                    fixture.label
                )
            });

        // Independent reconstruction: rebuild the ORIGINAL reflect::Fact
        // from nothing but the projection's retained (subject, ty), then
        // re-derive FactId via reflect's own frozen encoder — never reading
        // `derivation.id` until the final comparison.
        let rebuilt_fact = Fact::HasType {
            subject: retained.subject.clone(),
            ty: retained.ty.clone(),
            scope: ScopeId::root(),
        };
        let rebuilt_id = FactId::derive(&rebuilt_fact);

        assert_eq!(
            rebuilt_id, derivation.id,
            "{}: FactId-for-FactId shadow parity failed for {subject:?}",
            fixture.label
        );
        checked += 1;
    }
    checked
}

#[test]
fn has_type_fact_id_shadow_parity_across_the_corpus() {
    let mut total_checked = 0;
    for fixture in typecorpus::all_type_fixtures() {
        total_checked += assert_has_type_shadow_parity(&fixture);
    }
    assert!(
        total_checked > 0,
        "expected at least one HasType fact across the corpus to shadow-parity-check"
    );
}

// --- gate (c): ScopedWorldNonLeak ---

#[test]
fn scoped_world_non_leak_assumption_context_does_not_leak_to_parent() {
    let fixture = typecorpus::scoped_world_non_leak_probe();
    assert_eq!(fixture.category, ConformanceCategory::ScopedWorldNonLeak);

    let report = analyze(&fixture.source, &fixture.resolver);
    let regime = StructuralRegime::new();

    let root = ContextId::root();
    let child = root.extend(b"ScopedWorldNonLeak: hypothetical assumption");
    assert_ne!(child, root, "the child context must differ from root");

    let root_projection = regime.project(&report, root);
    let child_projection = regime.project(&report, child);

    assert!(
        !root_projection.has_type.is_empty(),
        "the probe fixture must contain at least one HasType fact"
    );
    assert_eq!(
        root_projection.has_type.len(),
        child_projection.has_type.len()
    );

    for (root_derived, child_derived) in root_projection
        .has_type
        .iter()
        .zip(child_projection.has_type.iter())
    {
        // Same underlying Realizes proposition (subject/ty/regime identical)
        // — only the context differs between the two projections.
        assert_eq!(root_derived.proposition, child_derived.proposition);

        // (i) the child judgement's ContextId is the child, not root.
        assert_eq!(child_derived.judgement.context, child);
        assert_ne!(child_derived.judgement.context, root);

        // Context alone must differentiate the two judgements.
        assert_ne!(root_derived.judgement.id(), child_derived.judgement.id());

        // (ii) the child's judgement must not appear in the parent (root)
        // context's projected judgement set — a fact derived under an
        // assumption does not leak to the parent context.
        assert!(
            !root_projection.judgement_ids.contains(&child_derived.judgement.id()),
            "a HasType judgement derived under an assumption scope leaked into the parent (root) projection"
        );
        // And symmetrically, the root judgement must not appear in the
        // child's own projected set under a different id either (sanity:
        // the two sets are genuinely disjoint here, not coincidentally
        // overlapping elsewhere).
        assert!(!child_projection
            .judgement_ids
            .contains(&root_derived.judgement.id()),);
    }
}
