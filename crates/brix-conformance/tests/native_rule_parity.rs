//! Differential parity test harness for native rule effect side-conditions (ADR-0009 §5 / N8b-1).

use std::collections::BTreeSet;

use brix_conformance::typecorpus::{self, RuleCategory};
use brix_ir::frontend::FrontendSource;
use brix_ir::reflect::{analyze as brix_analyze, ConflictKind};
use soc_regimes::native::{analyze as native_analyze, translate, NConflict};

const RULE_COVERAGE_FLOOR: usize = 3;

fn conflict_rule_category(kind: &ConflictKind) -> Option<RuleCategory> {
    match kind {
        ConflictKind::ImpureRule => Some(RuleCategory::Impure),
        ConflictKind::NondeterministicRule => Some(RuleCategory::Nondeterministic),
        ConflictKind::DivergentRule => Some(RuleCategory::Divergent),
        ConflictKind::UnboundHeadKey { .. } => Some(RuleCategory::UnboundHeadKey),
        ConflictKind::MaskRefNotEdgeBound { .. } => Some(RuleCategory::MaskRefNotEdgeBound),
        ConflictKind::OrdinaryFnOnDerivedRel { .. } => Some(RuleCategory::OrdinaryFnOnDerivedRel),
        _ => None,
    }
}

fn native_conflict_rule_category(conflict: &NConflict) -> Option<RuleCategory> {
    match conflict {
        NConflict::ImpureRule { .. } => Some(RuleCategory::Impure),
        NConflict::NondeterministicRule { .. } => Some(RuleCategory::Nondeterministic),
        NConflict::DivergentRule { .. } => Some(RuleCategory::Divergent),
        _ => None,
    }
}

#[test]
fn native_rule_effects_parity_corpus_coverage() {
    let fixtures = typecorpus::all_rule_fixtures();
    let mut covered = 0;

    let effects_axis = [
        RuleCategory::Impure,
        RuleCategory::Nondeterministic,
        RuleCategory::Divergent,
    ];

    for fixture in &fixtures {
        let source = FrontendSource {
            functions: vec![],
            rules: vec![fixture.rule.clone()],
            constraints: vec![],
            queries: vec![],
        };

        let brix_report = brix_analyze(&source, &fixture.resolver);
        let brix_cats: BTreeSet<RuleCategory> = brix_report
            .conflicts
            .iter()
            .filter_map(|c| conflict_rule_category(&c.kind))
            .collect();

        if brix_cats.iter().all(|cat| effects_axis.contains(cat)) {
            if let Some(native_src) = translate(&source, &fixture.resolver) {
                let native_report = native_analyze(&native_src);
                let native_cats: BTreeSet<RuleCategory> = native_report
                    .conflicts
                    .iter()
                    .filter_map(native_conflict_rule_category)
                    .collect();

                assert_eq!(
                    native_cats, brix_cats,
                    "Rule fixture category mismatch for '{}'",
                    fixture.label
                );
                assert_eq!(
                    native_cats, fixture.expected_categories,
                    "Rule fixture expected categories mismatch for '{}'",
                    fixture.label
                );
                covered += 1;
            }
        }
    }

    println!(
        "Rule effect side-condition coverage: {}/3 fixtures verified",
        covered
    );
    assert!(
        covered >= RULE_COVERAGE_FLOOR,
        "Rule coverage count {} < floor {}",
        covered,
        RULE_COVERAGE_FLOOR
    );
}
