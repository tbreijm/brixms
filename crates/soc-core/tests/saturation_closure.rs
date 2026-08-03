//! Stage D gate for divergence-sensitive saturation (ADR-0014 §8/§9, #61).
//!
//! Covers #61's acceptance criterion 7 — one-step closure holds for a sample
//! safety predicate — plus the CJ-1 adequacy interface Stage D states.
//!
//! The centrepiece is
//! [`the_acceptance_fixture_is_closed_under_visible_and_violated_under_raw`],
//! which is the ADR's own fixture. One graph settles four things at once: that
//! saturation genuinely hides, that hiding is semantically consequential, that
//! the mode distinction is real rather than decorative, and that the rule
//! detects violations at all.

use std::collections::BTreeMap;

use brix_canon::{CanonWriter, Digest, Domain};
use brix_semantic::{ConfigId, ContextId, Decomposition, GeneratorId};
use soc_core::adm::AdmAll;
use soc_core::calendar::Key;
use soc_core::commit::SettlementRegime;
use soc_core::exec::ExecConfig;
use soc_core::history::History;
use soc_core::intern::{Handle, Interner};
use soc_core::regime::{Candidate, Regime};
use soc_core::saturate::{
    adequacy_of, check_closure, fo_definedness, run_saturated, sat_step, ClosureMode,
    ClosureResult, ClosureUnknown, DeclaredAssumptions, FoDefinedness, FoUndefined, FoValue,
    GeneratorPartitionProfile, ObservationProfile, PresentationIdV1, PresentationV1, SafetyState,
    SaturationBudget, ViolationSite,
};

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

struct Edge {
    witness: Handle,
    successor: Handle,
    generators: Vec<GeneratorId>,
}

struct ChainRegime {
    id: Handle,
    edges: BTreeMap<Handle, Edge>,
    configs: BTreeMap<Handle, ConfigId>,
}

impl Regime for ChainRegime {
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        self.edges
            .get(&e.world)
            .map(|edge| {
                vec![Candidate {
                    regime: self.id,
                    witness: edge.witness,
                    successor: edge.successor,
                }]
            })
            .unwrap_or_default()
    }
}

impl SettlementRegime for ChainRegime {
    fn decompose(&self, e: &ExecConfig, c: &Candidate) -> Decomposition {
        let edge = self.edges.get(&e.world).expect("decompose on a live edge");
        Decomposition::recorded(
            edge.generators.clone(),
            vec![self.configs[&e.world], self.configs[&c.successor]],
        )
        .expect("well-formed decomposition")
    }
}

fn tag(i: &mut Interner, s: &str) -> Handle {
    i.intern(Digest::of(Domain::Value, s.as_bytes()))
}

fn tiebreak_of(c: &Candidate) -> Digest {
    let mut w = CanonWriter::new();
    w.write_uint(c.witness.raw() as u64);
    w.write_uint(c.successor.raw() as u64);
    w.digest(Domain::Value)
}

fn gen_tau() -> GeneratorId {
    GeneratorId::named("closure-fixture.tau@1")
}
fn gen_realizing() -> GeneratorId {
    GeneratorId::named("closure-fixture.realizing@1")
}

fn hiding_profile() -> GeneratorPartitionProfile {
    GeneratorPartitionProfile::new(
        [gen_tau()].into_iter().collect(),
        [gen_realizing()].into_iter().collect(),
    )
    .expect("disjoint partitions")
}

struct Fixture {
    interner: Interner,
    regime: ChainRegime,
    worlds: BTreeMap<&'static str, Handle>,
    policy: Handle,
}

fn build_fixture(spec: &[(&'static str, &'static str, Vec<GeneratorId>)]) -> Fixture {
    let mut interner = Interner::new();
    let mut worlds: BTreeMap<&'static str, Handle> = BTreeMap::new();
    for (from, to, _) in spec {
        for name in [from, to] {
            if !worlds.contains_key(name) {
                worlds.insert(name, tag(&mut interner, name));
            }
        }
    }
    let policy = tag(&mut interner, "closure.policy");
    let regime_id = tag(&mut interner, "closure.regime");

    let configs = worlds
        .values()
        .map(|h| (*h, ConfigId(interner.resolve(*h))))
        .collect();

    let mut edges = BTreeMap::new();
    for (from, to, generators) in spec {
        let witness = tag(&mut interner, &format!("closure.witness.{from}->{to}"));
        edges.insert(
            worlds[from],
            Edge {
                witness,
                successor: worlds[to],
                generators: generators.clone(),
            },
        );
    }

    Fixture {
        interner,
        regime: ChainRegime {
            id: regime_id,
            edges,
            configs,
        },
        worlds,
        policy,
    }
}

impl Fixture {
    fn exec_at(&self, world: &str) -> ExecConfig {
        ExecConfig::new(self.worlds[world], self.policy, History::empty().digest())
    }

    fn config(&self, world: &str) -> ConfigId {
        ConfigId(self.interner.resolve(self.worlds[world]))
    }
}

fn presentation<'a>(
    regimes: &'a [&'a dyn SettlementRegime],
    profile: &'a dyn ObservationProfile,
    interner: &'a Interner,
) -> PresentationV1<'a> {
    PresentationV1 {
        id: PresentationIdV1::from_canon(b"closure-fixture@1"),
        regimes,
        regime_set: Digest::of(Domain::Value, b"closure.regime-set"),
        adm: &AdmAll,
        adm_id: Digest::of(Domain::Value, b"closure.adm-all"),
        profile,
        interner,
        context: ContextId::root(),
        assumptions: DeclaredAssumptions::all(),
    }
}

fn keyer() -> impl FnMut(&Candidate, u64) -> Key {
    |c: &Candidate, phase: u64| Key::new(phase, 0, tiebreak_of(c))
}

fn budget() -> SaturationBudget {
    SaturationBudget::uniform(64)
}

/// The ADR's acceptance graph: `w0 -τ→ w_bad -τ→ w1 -o→ w2`, terminal at `w2`.
fn acceptance_fixture() -> Fixture {
    build_fixture(&[
        ("w0", "w_bad", vec![gen_tau()]),
        ("w_bad", "w1", vec![gen_tau()]),
        ("w1", "w2", vec![gen_realizing()]),
    ])
}

// ---------------------------------------------------------------------------
// AC-7 — the acceptance fixture.
// ---------------------------------------------------------------------------

/// **The Stage D acceptance fixture** (ADR-0014 §8). `Φ = (world ≠ w_bad)` over
/// `w0 -τ→ w_bad -τ→ w1 -o→ w2`.
///
/// Under `Visible` the predicate is closed: an observer at the declared
/// boundary never sees `w_bad`, because saturation hides the whole
/// administrative prefix. Under `Raw` it is violated at exactly that state.
/// Both are correct answers to *different questions*, which is why the mode is
/// a required argument rather than a default.
#[test]
fn the_acceptance_fixture_is_closed_under_visible_and_violated_under_raw() {
    let fx = acceptance_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner);
    let bad = fx.config("w_bad");
    let predicate = move |s: &SafetyState| s.world != bad;

    let mut k = keyer();
    let visible = check_closure(
        &pres,
        fx.exec_at("w0"),
        &predicate,
        ClosureMode::Visible,
        &mut k,
        budget(),
    );
    assert!(
        visible.is_closed(),
        "the boundary never exposes w_bad, so Φ is a visible invariant: {visible:?}"
    );

    let mut k = keyer();
    let raw = check_closure(
        &pres,
        fx.exec_at("w0"),
        &predicate,
        ClosureMode::Raw,
        &mut k,
        budget(),
    );
    let ClosureResult::Violated(ref violation) = raw else {
        panic!("the system really does pass through w_bad: {raw:?}");
    };
    assert_eq!(violation.mode, ClosureMode::Raw);
    assert_eq!(violation.site, ViolationSite::AdministrativeIntermediate);
    assert_eq!(violation.state.world, bad);
    assert_eq!(
        violation.visible_depth, 0,
        "the violation happens inside the first saturated step"
    );

    assert_ne!(
        visible.is_closed(),
        raw.is_closed(),
        "if the two modes agreed here the distinction would be decorative"
    );
}

/// A genuine closure failure — the predicate holds before a realizing step and
/// not after it — must be caught under `Visible` too. Otherwise the previous
/// test would only be showing that `Visible` ignores things.
#[test]
fn a_violation_at_a_visible_successor_is_caught_under_visible() {
    let fx = build_fixture(&[
        ("w0", "w1", vec![gen_realizing()]),
        ("w1", "w_bad", vec![gen_realizing()]),
    ]);
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner);
    let bad = fx.config("w_bad");
    let predicate = move |s: &SafetyState| s.world != bad;

    let mut k = keyer();
    let result = check_closure(
        &pres,
        fx.exec_at("w0"),
        &predicate,
        ClosureMode::Visible,
        &mut k,
        budget(),
    );

    let ClosureResult::Violated(violation) = result else {
        panic!("Φ is broken by the second visible step: {result:?}");
    };
    assert_eq!(violation.site, ViolationSite::VisibleSuccessor);
    assert_eq!(violation.state.world, bad);
    assert_eq!(
        violation.visible_depth, 2,
        "the second realizing step is the one that breaks it"
    );
}

/// A predicate that fails at the start was never an invariant, and saying so is
/// different from saying a step broke it.
#[test]
fn a_predicate_false_at_the_initial_state_is_reported_as_such() {
    let fx = acceptance_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner);
    let start = fx.config("w0");
    let predicate = move |s: &SafetyState| s.world != start;

    let mut k = keyer();
    let result = check_closure(
        &pres,
        fx.exec_at("w0"),
        &predicate,
        ClosureMode::Visible,
        &mut k,
        budget(),
    );

    let ClosureResult::Violated(violation) = result else {
        panic!("expected an initial-state violation, got {result:?}");
    };
    assert_eq!(violation.site, ViolationSite::Initial);
    assert_eq!(violation.visible_depth, 0);
}

/// Certified divergence still decides closure: the lasso repeats states already
/// checked, so nothing unchecked remains. This is a payoff from Stage B
/// certifying divergence rather than merely bounding it — an exhausted budget
/// could not support the same conclusion, as the next test shows.
#[test]
fn a_certified_lasso_still_decides_closure() {
    let fx = build_fixture(&[("w0", "w1", vec![gen_tau()]), ("w1", "w0", vec![gen_tau()])]);
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner);
    let absent = ConfigId::from_canon(b"a world this fixture never reaches");
    let predicate = move |s: &SafetyState| s.world != absent;

    let mut k = keyer();
    let result = check_closure(
        &pres,
        fx.exec_at("w0"),
        &predicate,
        ClosureMode::Raw,
        &mut k,
        budget(),
    );

    assert!(
        result.is_closed(),
        "every state the orbit will ever reach has been checked: {result:?}"
    );
}

/// …and an *exhausted* run cannot. The reachable set is unknown, so there is no
/// invariant claim to make — never `Closed`.
#[test]
fn an_exhausted_run_yields_unknown_never_closed() {
    let fx = build_fixture(&[
        ("w0", "w1", vec![gen_tau()]),
        ("w1", "w2", vec![gen_tau()]),
        ("w2", "w3", vec![gen_tau()]),
        ("w3", "w4", vec![gen_realizing()]),
    ]);
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner);
    let absent = ConfigId::from_canon(b"a world this fixture never reaches");
    let predicate = move |s: &SafetyState| s.world != absent;

    let mut k = keyer();
    let result = check_closure(
        &pres,
        fx.exec_at("w0"),
        &predicate,
        ClosureMode::Visible,
        &mut k,
        SaturationBudget {
            max_hidden_steps: 1,
            max_administrative_states: 64,
            max_visible_steps: 64,
        },
    );

    assert!(!result.is_closed());
    match result {
        ClosureResult::Unknown(ClosureUnknown::Unestablished { visible_depth, .. }) => {
            assert_eq!(visible_depth, 0)
        }
        other => panic!("an unexplored reachable set must be Unknown, got {other:?}"),
    }
}

/// A predicate that holds everywhere is closed, and the walk reports how much
/// it actually looked at — so a vacuous pass over zero states is not
/// indistinguishable from a real one.
#[test]
fn a_true_predicate_is_closed_and_reports_its_coverage() {
    let fx = acceptance_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner);

    let mut k = keyer();
    let result = check_closure(
        &pres,
        fx.exec_at("w0"),
        &|_| true,
        ClosureMode::Raw,
        &mut k,
        budget(),
    );

    match result {
        ClosureResult::Closed {
            states_checked,
            visible_steps,
        } => {
            // The initial state plus the three committed destinations.
            assert_eq!(states_checked, 4);
            assert_eq!(visible_steps, 1);
        }
        other => panic!("expected Closed, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The CJ-1 adequacy interface.
// ---------------------------------------------------------------------------

/// Interface property 3: the answer *is* the encoded `F_O`-structure, and the
/// classification of every summand is total and exhaustive.
#[test]
fn every_saturated_summand_classifies_against_the_fo_sub_carrier() {
    let terminating = acceptance_fixture();
    let looping = build_fixture(&[("w0", "w1", vec![gen_tau()]), ("w1", "w0", vec![gen_tau()])]);
    let profile = hiding_profile();

    let t_regime: &dyn SettlementRegime = &terminating.regime;
    let t_regimes = std::slice::from_ref(&t_regime);
    let t_pres = presentation(t_regimes, &profile, &terminating.interner);
    let l_regime: &dyn SettlementRegime = &looping.regime;
    let l_regimes = std::slice::from_ref(&l_regime);
    let l_pres = presentation(l_regimes, &profile, &looping.interner);

    // Realizing — inside the sub-carrier.
    let mut k = keyer();
    let (realizing, _, _) = sat_step(&t_pres, &terminating.exec_at("w0"), 0, &mut k, budget());
    assert_eq!(
        fo_definedness(&realizing),
        FoDefinedness::Defined(FoValue::Realizing)
    );

    // Quiescent — inside.
    let mut k = keyer();
    let (quiescent, _, _) = sat_step(&t_pres, &terminating.exec_at("w2"), 0, &mut k, budget());
    assert_eq!(
        fo_definedness(&quiescent),
        FoDefinedness::Defined(FoValue::Quiescent)
    );

    // Certified divergence — outside, and known to be.
    let mut k = keyer();
    let (divergent, _, _) = sat_step(&l_pres, &looping.exec_at("w0"), 0, &mut k, budget());
    assert_eq!(
        fo_definedness(&divergent),
        FoDefinedness::Undefined(FoUndefined::CertifiedDivergence)
    );

    // Exhaustion — outside, and *not* known to be. Keeping these two apart is
    // the whole point: collapsing them would let a resource limit masquerade
    // as a semantic fact.
    let mut k = keyer();
    let (unknown, _, _) = sat_step(
        &t_pres,
        &terminating.exec_at("w0"),
        0,
        &mut k,
        SaturationBudget {
            max_hidden_steps: 0,
            max_administrative_states: 64,
            max_visible_steps: 64,
        },
    );
    assert_eq!(
        fo_definedness(&unknown),
        FoDefinedness::Undefined(FoUndefined::Unestablished)
    );
    assert_ne!(fo_definedness(&unknown), fo_definedness(&divergent));
}

#[test]
fn a_terminating_run_is_fo_defined_throughout_and_a_diverging_one_is_not() {
    let terminating = acceptance_fixture();
    let looping = build_fixture(&[
        ("w0", "w1", vec![gen_realizing()]),
        ("w1", "w2", vec![gen_tau()]),
        ("w2", "w1", vec![gen_tau()]),
    ]);
    let profile = hiding_profile();

    let t_regime: &dyn SettlementRegime = &terminating.regime;
    let t_regimes = std::slice::from_ref(&t_regime);
    let t_pres = presentation(t_regimes, &profile, &terminating.interner);
    let mut k = keyer();
    let t_run = run_saturated(&t_pres, terminating.exec_at("w0"), &mut k, budget());
    let t_report = adequacy_of(&t_run);
    assert!(t_report.defined_throughout);
    assert_eq!(t_report.outcome, FoDefinedness::Defined(FoValue::Quiescent));
    assert_eq!(t_report.left_at_visible_depth, None);

    let l_regime: &dyn SettlementRegime = &looping.regime;
    let l_regimes = std::slice::from_ref(&l_regime);
    let l_pres = presentation(l_regimes, &profile, &looping.interner);
    let mut k = keyer();
    let l_run = run_saturated(&l_pres, looping.exec_at("w0"), &mut k, budget());
    let l_report = adequacy_of(&l_run);
    assert!(!l_report.defined_throughout);
    assert_eq!(
        l_report.outcome,
        FoDefinedness::Undefined(FoUndefined::CertifiedDivergence)
    );
    assert_eq!(
        l_report.left_at_visible_depth,
        Some(1),
        "it left the sub-carrier after one visible step"
    );
}
