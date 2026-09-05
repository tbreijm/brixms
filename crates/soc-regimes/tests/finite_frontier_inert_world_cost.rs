//! Tests for declared Footprint, inert configuration skipping, and O(|Δ|) step cost invariance (ADR-0002 §9.1).

use brix_semantic::ConfigId;
use soc_core::delta::{CandidateDelta, Delta, Footprint};
use soc_core::engine::{IncrementalEngine, IncrementalWitnessIndex};
use soc_core::intern::Interner;
use soc_regimes::finite_frontier::FiniteCandidateRegime;

struct InertProvider;

impl IncrementalWitnessIndex for InertProvider {
    fn footprint(&self) -> Footprint {
        Footprint::empty()
    }
    fn apply(&mut self, _delta: &Delta) -> CandidateDelta {
        CandidateDelta::new()
    }
}

#[test]
fn footprint_matches_registered_source_configuration_handles() {
    let mut interner = Interner::new();
    let mut regime = FiniteCandidateRegime::default_regime(&mut interner);

    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");
    let cfg_c = ConfigId::from_canon(b"cfg-C");

    let c1 = regime.register_named_candidate(&mut interner, "c1".to_string(), cfg_a, cfg_b, 1);
    let c2 = regime.register_named_candidate(&mut interner, "c2".to_string(), cfg_b, cfg_c, 2);

    match regime.footprint() {
        Footprint::Configs(set) => {
            assert_eq!(set.len(), 2);
            assert!(set.contains(&c1.src_handle));
            assert!(set.contains(&c2.src_handle));
            assert!(!set.contains(&c2.successor_handle));
        }
        Footprint::AllConfigs => {
            panic!("FiniteCandidateRegime must declare an explicit config footprint")
        }
    }
}

#[test]
fn inert_configuration_delta_is_zero_work_skip() {
    let mut interner = Interner::new();
    let mut regime = FiniteCandidateRegime::default_regime(&mut interner);

    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");
    regime.register_named_candidate(&mut interner, "c1".to_string(), cfg_a, cfg_b, 1);

    let inert_cfg = ConfigId::from_canon(b"inert-cfg");
    let inert_handle = interner.intern(inert_cfg.digest());

    let mut engine = IncrementalEngine::new(vec![Box::new(regime)]);

    let report = engine.step(&Delta::of_added([inert_handle]));
    assert!(report.candidate_delta.is_empty());
    assert_eq!(
        report.cost.work_units(),
        Some(1),
        "Only the single index lookup is paid for an inert config delta"
    );
    assert!(engine.view().is_empty());
}

#[test]
fn step_cost_is_unaffected_by_inert_provider_scaling() {
    let mut interner = Interner::new();
    let mut regime_template = FiniteCandidateRegime::default_regime(&mut interner);

    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");
    let c1 =
        regime_template.register_named_candidate(&mut interner, "c1".to_string(), cfg_a, cfg_b, 1);

    // Lean engine: active regime only
    let mut lean_engine = IncrementalEngine::new(vec![Box::new(regime_template.clone())]);

    // Ballasted engine: active regime + 1000 inert providers
    let mut ballasted_providers: Vec<Box<dyn IncrementalWitnessIndex>> =
        vec![Box::new(regime_template)];
    for _ in 0..1000 {
        ballasted_providers.push(Box::new(InertProvider));
    }
    let mut ballasted_engine = IncrementalEngine::new(ballasted_providers);

    let delta = Delta::of_added([c1.src_handle]);

    let lean_cost = lean_engine.step(&delta).cost.work_units().unwrap();
    let ballasted_cost = ballasted_engine.step(&delta).cost.work_units().unwrap();

    assert_eq!(
        lean_cost, ballasted_cost,
        "Per-step incremental cost must be provably independent of inert provider ballast (O(|Δ|) invariant)"
    );
}
