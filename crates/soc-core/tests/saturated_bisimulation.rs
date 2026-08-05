//! Stage C gate for divergence-sensitive saturation (ADR-0014 §9, #61).
//!
//! Covers #61's acceptance criteria 5 (incremental and naive saturated visible
//! behavior agree) and 6 (weak bisimulation with a minimal counterexample),
//! plus the ⟨D-REF⟩ pin that refinement's asymmetry is real in exactly one
//! direction.
//!
//! Two claims here carry the most weight:
//!
//! - [`an_observation_mismatch_at_visible_depth_two_yields_a_prefix_of_exactly_two`]
//!   asserts counterexample minimality **exactly**, not approximately. Because
//!   `F_O` is deterministic there is one path from each start pair, so the
//!   prefix at the first mismatch is the unique shortest disagreeing visible
//!   trace by construction — no search, no shrinking (ADR-0014 §7.3).
//! - [`a_terminating_impl_refines_a_diverging_spec_but_is_not_bisimilar_to_it`]
//!   runs **both** contracts over one pair of systems, proving they are
//!   genuinely different rather than one being a spelling of the other.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonWriter, Digest, Domain};
use brix_semantic::{ConfigId, ContextId, Decomposition, GeneratorId};
use soc_core::adm::AdmAll;
use soc_core::calendar::Key;
use soc_core::commit::{CommitError, SettlementRegime};
use soc_core::delta::{CandidateDelta, Delta, Footprint};
use soc_core::engine::{naive_view_over, IncrementalEngine, IncrementalRegime};
use soc_core::exec::ExecConfig;
use soc_core::history::History;
use soc_core::intern::{Handle, Interner};
use soc_core::regime::{Candidate, Regime};
use soc_core::saturate::{
    check_saturated, run_saturated, AssumptionId, ComparisonUnknown, Contract, DeclaredAssumptions,
    GeneratorPartitionProfile, MismatchKind, ObservationProfile, PresentationIdV1, PresentationV1,
    PresentedSystem, SaturatedComparison, SaturatedStop, SaturationBudget, SaturationUnknown,
    Summand,
};

// ---------------------------------------------------------------------------
// Fixture — the edge-table regime idiom, with a per-edge generator so an
// observation profile can partition the graph.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Edge {
    witness: Handle,
    successor: Handle,
    generators: Vec<GeneratorId>,
}

#[derive(Clone)]
struct ChainRegime {
    id: Handle,
    edges: BTreeMap<Handle, Edge>,
    configs: BTreeMap<Handle, ConfigId>,
}

impl ChainRegime {
    fn candidate_at(&self, world: Handle) -> Option<Candidate> {
        self.edges.get(&world).map(|edge| Candidate {
            regime: self.id,
            witness: edge.witness,
            successor: edge.successor,
        })
    }

    fn decomposition(&self, world: Handle, successor: Handle) -> Decomposition {
        let edge = self
            .edges
            .get(&world)
            .expect("try_decompose on a live edge");
        Decomposition::recorded(
            edge.generators.clone(),
            vec![self.configs[&world], self.configs[&successor]],
        )
        .expect("well-formed decomposition")
    }
}

impl Regime for ChainRegime {
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        self.candidate_at(e.world).into_iter().collect()
    }
}

impl SettlementRegime for ChainRegime {
    fn try_decompose(&self, e: &ExecConfig, c: &Candidate) -> Result<Decomposition, CommitError> {
        Ok(self.decomposition(e.world, c.successor))
    }
}

impl IncrementalRegime for ChainRegime {
    fn footprint(&self) -> Footprint {
        Footprint::configs(self.edges.keys().copied())
    }

    fn apply(&mut self, delta: &Delta) -> CandidateDelta {
        let mut out = CandidateDelta::new();
        for h in &delta.added {
            if let Some(c) = self.candidate_at(*h) {
                out.added.insert(c);
            }
        }
        for h in &delta.removed {
            if let Some(c) = self.candidate_at(*h) {
                out.removed.insert(c);
            }
        }
        out
    }
}

/// The naive candidate source: `naive_view_over` recomputed from scratch over
/// the singleton presented set `{e.world}`.
///
/// Deliberately routed through the *same* function `incremental_differential.rs`
/// uses as the Step 6 reference, so the saturated parity claim below is anchored
/// to the same baseline as the unsaturated one. `naive_view_over` applies `adm`
/// and `commit_tick` applies it again; filtering twice is idempotent.
struct NaiveSource {
    inner: ChainRegime,
}

impl Regime for NaiveSource {
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        let regime: &dyn Regime = &self.inner;
        let present = BTreeSet::from([e.world]);
        naive_view_over(
            std::slice::from_ref(&regime),
            &AdmAll,
            &present,
            e.policy,
            e.history,
        )
        .into_iter()
        .collect()
    }
}

impl SettlementRegime for NaiveSource {
    fn try_decompose(&self, e: &ExecConfig, c: &Candidate) -> Result<Decomposition, CommitError> {
        Ok(self.inner.decomposition(e.world, c.successor))
    }
}

/// The incremental candidate source: an [`IncrementalEngine`] whose presented
/// set is kept at exactly `{e.world}`, answering from its materialized view.
///
/// `Regime::candidates` takes `&self`, so the engine sits behind a `RefCell` and
/// syncs lazily. That is a fixture concern, not a production one: what is being
/// tested is that the *view* and the *recompute* drive saturation identically,
/// and the sync is how a single-world driver presents itself to a set-shaped
/// engine.
struct IncrementalSource {
    engine: RefCell<IncrementalEngine>,
    presented: Cell<Option<Handle>>,
    shape: ChainRegime,
}

impl IncrementalSource {
    fn new(shape: ChainRegime) -> Self {
        Self {
            engine: RefCell::new(IncrementalEngine::new(vec![Box::new(shape.clone())])),
            presented: Cell::new(None),
            shape,
        }
    }
}

impl Regime for IncrementalSource {
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        if self.presented.get() != Some(e.world) {
            let delta = match self.presented.get() {
                Some(old) => Delta::between_worlds(old, e.world),
                None => Delta::of_added([e.world]),
            };
            self.engine.borrow_mut().step(&delta);
            self.presented.set(Some(e.world));
        }
        self.engine.borrow().view().iter().copied().collect()
    }
}

impl SettlementRegime for IncrementalSource {
    fn try_decompose(&self, e: &ExecConfig, c: &Candidate) -> Result<Decomposition, CommitError> {
        Ok(self.shape.decomposition(e.world, c.successor))
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
    GeneratorId::named("bisim-fixture.tau@1")
}
fn gen_o(tag: &str) -> GeneratorId {
    GeneratorId::named(&format!("bisim-fixture.realizing.{tag}@1"))
}

fn hiding_profile() -> GeneratorPartitionProfile {
    GeneratorPartitionProfile::new(
        [gen_tau()].into_iter().collect(),
        ["a", "b", "c", "d"].into_iter().map(gen_o).collect(),
    )
    .expect("disjoint partitions")
}

struct Fixture {
    interner: Interner,
    regime: ChainRegime,
    worlds: BTreeMap<&'static str, Handle>,
    policy: Handle,
}

/// Each spec entry is `(from, to, generators)`. World *names* are interned as
/// canonical digests, so two fixtures that name the same world produce the same
/// `ConfigId` even with different interners — which is what lets two systems be
/// compared at all.
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
    let policy = tag(&mut interner, "bisim.policy");
    let regime_id = tag(&mut interner, "bisim.regime");

    let configs = worlds
        .values()
        .map(|h| (*h, ConfigId(interner.resolve(*h))))
        .collect();

    let mut edges = BTreeMap::new();
    for (from, to, generators) in spec {
        // The witness handle is named by the *generators*, not by the endpoints,
        // so two graphs whose τ-prefixes differ still agree on the realizing
        // step they share.
        let witness = tag(&mut interner, &format!("bisim.witness.{from}->{to}"));
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
}

fn presentation<'a>(
    seed: &[u8],
    regimes: &'a [&'a dyn SettlementRegime],
    profile: &'a dyn ObservationProfile,
    interner: &'a Interner,
    assumptions: DeclaredAssumptions,
) -> PresentationV1<'a> {
    PresentationV1 {
        id: PresentationIdV1::from_canon(seed),
        regimes,
        regime_set: Digest::of(Domain::Value, b"bisim.regime-set"),
        adm: &AdmAll,
        adm_id: Digest::of(Domain::Value, b"bisim.adm-all"),
        profile,
        interner,
        context: ContextId::root(),
        assumptions,
    }
}

fn keyer() -> impl FnMut(&Candidate, u64) -> Key {
    |c: &Candidate, phase: u64| Key::new(phase, 0, tiebreak_of(c))
}

fn budget() -> SaturationBudget {
    SaturationBudget::uniform(64)
}

// ---------------------------------------------------------------------------
// `run_saturated` — every exit is a claim.
// ---------------------------------------------------------------------------

/// `w0 -τ-> w1 -o(a)-> w2 -τ-> w3 -o(b)-> w4`, terminal at `w4`.
fn mixed_fixture() -> Fixture {
    build_fixture(&[
        ("w0", "w1", vec![gen_tau()]),
        ("w1", "w2", vec![gen_o("a")]),
        ("w2", "w3", vec![gen_tau()]),
        ("w3", "w4", vec![gen_o("b")]),
    ])
}

#[test]
fn a_saturated_run_exports_only_visible_steps_but_journals_everything() {
    let fx = mixed_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(
        b"run@1",
        regimes,
        &profile,
        &fx.interner,
        DeclaredAssumptions::all(),
    );
    let mut k = keyer();

    let run = run_saturated(&pres, fx.exec_at("w0"), &mut k, budget());

    assert_eq!(run.visible.len(), 2, "two realizing steps are exported");
    assert_eq!(
        run.journal.len(),
        4,
        "all four committed steps are journaled — τ steps are evidence, not noise"
    );
    assert!(
        run.stop.is_quiescent(),
        "the run must end in certified quiescence, got {:?}",
        run.stop
    );
    let SaturatedStop::Quiescent(cert) = &run.stop else {
        unreachable!()
    };
    assert_eq!(
        cert.terminal_world,
        ConfigId(fx.interner.resolve(fx.worlds["w4"]))
    );
}

#[test]
fn a_looping_run_stops_divergent_not_quiescent() {
    let fx = build_fixture(&[
        ("w0", "w1", vec![gen_o("a")]),
        ("w1", "w2", vec![gen_tau()]),
        ("w2", "w1", vec![gen_tau()]),
    ]);
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(
        b"run@1",
        regimes,
        &profile,
        &fx.interner,
        DeclaredAssumptions::all(),
    );
    let mut k = keyer();

    let run = run_saturated(&pres, fx.exec_at("w0"), &mut k, budget());

    assert_eq!(run.visible.len(), 1, "the one realizing step is exported");
    assert!(!run.stop.is_quiescent());
    match &run.stop {
        SaturatedStop::Divergent(cert) => assert_eq!(cert.cycle, 2),
        other => panic!("expected certified divergence, got {other:?}"),
    }
}

/// The whole-run budget is the last place the settled-versus-exhausted
/// conflation could have survived. Stage A declared this variant and could not
/// reach it; the driver makes it reachable, and it is emphatically not a stop.
#[test]
fn the_visible_budget_is_an_explicit_unknown_never_a_quiescent_stop() {
    let fx = mixed_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(
        b"run@1",
        regimes,
        &profile,
        &fx.interner,
        DeclaredAssumptions::all(),
    );
    let mut k = keyer();

    let run = run_saturated(
        &pres,
        fx.exec_at("w0"),
        &mut k,
        SaturationBudget {
            max_hidden_steps: 64,
            max_administrative_states: 64,
            max_visible_steps: 1,
        },
    );

    assert_eq!(run.visible.len(), 1);
    assert!(!run.stop.is_quiescent());
    assert_eq!(
        run.stop,
        SaturatedStop::Unknown(SaturationUnknown::VisibleBudgetExhausted {
            visible_steps: 1,
            budget: 1,
        })
    );
}

#[test]
fn saturated_runs_are_deterministic() {
    let fx = mixed_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(
        b"run@1",
        regimes,
        &profile,
        &fx.interner,
        DeclaredAssumptions::all(),
    );

    let mut k1 = keyer();
    let first = run_saturated(&pres, fx.exec_at("w0"), &mut k1, budget());
    let mut k2 = keyer();
    let second = run_saturated(&pres, fx.exec_at("w0"), &mut k2, budget());

    assert_eq!(first.visible, second.visible);
    assert_eq!(first.chain_digest(), second.chain_digest());
    assert_eq!(first.stop, second.stop);
}

// ---------------------------------------------------------------------------
// AC-5 — naive and incremental saturated behavior agree.
// ---------------------------------------------------------------------------

/// SOC-LAW-08's parity, stated at the saturated level. The contract is
/// **`Bisimilar`, not `Refines`** (ADR-0014 §7.2, normative): the fast engine
/// must be identical to the reference oracle, not merely a refinement of it.
#[test]
fn the_incremental_and_naive_candidate_sources_are_saturated_bisimilar() {
    let fx = mixed_fixture();
    let profile = hiding_profile();

    let naive = NaiveSource {
        inner: fx.regime.clone(),
    };
    let incremental = IncrementalSource::new(fx.regime.clone());

    let naive_regime: &dyn SettlementRegime = &naive;
    let naive_regimes = std::slice::from_ref(&naive_regime);
    let incremental_regime: &dyn SettlementRegime = &incremental;
    let incremental_regimes = std::slice::from_ref(&incremental_regime);

    let mut naive_system = PresentedSystem::new(
        presentation(
            b"naive@1",
            naive_regimes,
            &profile,
            &fx.interner,
            DeclaredAssumptions::all(),
        ),
        fx.exec_at("w0"),
        keyer(),
        budget(),
    );
    let mut incremental_system = PresentedSystem::new(
        presentation(
            b"incremental@1",
            incremental_regimes,
            &profile,
            &fx.interner,
            DeclaredAssumptions::all(),
        ),
        fx.exec_at("w0"),
        keyer(),
        budget(),
    );

    let result = check_saturated(
        &mut incremental_system,
        &mut naive_system,
        Contract::Bisimilar,
        64,
    );

    assert!(
        result.holds(),
        "the incremental view and the naive recompute must drive saturation \
         identically, got {result:?}"
    );
    match result {
        SaturatedComparison::Holds { visible_steps, .. } => assert_eq!(visible_steps, 2),
        other => panic!("expected Holds, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// AC-6 — weak bisimulation, minimal counterexamples, direction.
// ---------------------------------------------------------------------------

/// Different τ-layouts, identical visible behavior. `τ;τ;o` on one side and
/// `τ;o` on the other reach the same world and fire the same realizing edge, so
/// saturation hides the difference — which is the entire point of hiding.
#[test]
fn different_administrative_layouts_with_equal_visible_behavior_are_bisimilar() {
    let long = build_fixture(&[
        ("w0", "a1", vec![gen_tau()]),
        ("a1", "m", vec![gen_tau()]),
        ("m", "end", vec![gen_o("a")]),
    ]);
    let short = build_fixture(&[("w0", "m", vec![gen_tau()]), ("m", "end", vec![gen_o("a")])]);
    let profile = hiding_profile();

    let long_regime: &dyn SettlementRegime = &long.regime;
    let long_regimes = std::slice::from_ref(&long_regime);
    let short_regime: &dyn SettlementRegime = &short.regime;
    let short_regimes = std::slice::from_ref(&short_regime);

    let mut long_system = PresentedSystem::new(
        presentation(
            b"long@1",
            long_regimes,
            &profile,
            &long.interner,
            DeclaredAssumptions::all(),
        ),
        long.exec_at("w0"),
        keyer(),
        budget(),
    );
    let mut short_system = PresentedSystem::new(
        presentation(
            b"short@1",
            short_regimes,
            &profile,
            &short.interner,
            DeclaredAssumptions::all(),
        ),
        short.exec_at("w0"),
        keyer(),
        budget(),
    );

    let result = check_saturated(&mut long_system, &mut short_system, Contract::Bisimilar, 64);
    assert!(
        result.holds(),
        "τ;τ;o and τ;o with the same visible step must be bisimilar, got {result:?}"
    );

    // …and the journals differ, which is exactly what must NOT be compared.
    let mut k1 = keyer();
    let long_run = run_saturated(
        long_system.presentation(),
        long.exec_at("w0"),
        &mut k1,
        budget(),
    );
    let mut k2 = keyer();
    let short_run = run_saturated(
        short_system.presentation(),
        short.exec_at("w0"),
        &mut k2,
        budget(),
    );
    assert_eq!(long_run.visible, short_run.visible);
    assert_ne!(
        long_run.journal.len(),
        short_run.journal.len(),
        "the administrative layouts really do differ"
    );
    assert_ne!(
        long_run.chain_digest(),
        short_run.chain_digest(),
        "chain equality is the WRONG parity notion for #61 (ADR-0014 risk 3)"
    );
}

/// Minimality asserted exactly. Two graphs agree on visible steps 0 and 1 and
/// disagree on step 2, so the counterexample's prefix is **exactly** two
/// observations — no more (which would mean it walked past the disagreement)
/// and no fewer (which would mean it reported a disagreement that had not
/// happened yet).
#[test]
fn an_observation_mismatch_at_visible_depth_two_yields_a_prefix_of_exactly_two() {
    let left = build_fixture(&[
        ("w0", "w1", vec![gen_o("a")]),
        ("w1", "w2", vec![gen_o("b")]),
        ("w2", "w3", vec![gen_o("c")]),
    ]);
    let right = build_fixture(&[
        ("w0", "w1", vec![gen_o("a")]),
        ("w1", "w2", vec![gen_o("b")]),
        ("w2", "w3", vec![gen_o("d")]),
    ]);
    let profile = hiding_profile();

    let left_regime: &dyn SettlementRegime = &left.regime;
    let left_regimes = std::slice::from_ref(&left_regime);
    let right_regime: &dyn SettlementRegime = &right.regime;
    let right_regimes = std::slice::from_ref(&right_regime);

    let mut left_system = PresentedSystem::new(
        presentation(
            b"left@1",
            left_regimes,
            &profile,
            &left.interner,
            DeclaredAssumptions::all(),
        ),
        left.exec_at("w0"),
        keyer(),
        budget(),
    );
    let mut right_system = PresentedSystem::new(
        presentation(
            b"right@1",
            right_regimes,
            &profile,
            &right.interner,
            DeclaredAssumptions::all(),
        ),
        right.exec_at("w0"),
        keyer(),
        budget(),
    );

    let result = check_saturated(&mut left_system, &mut right_system, Contract::Bisimilar, 64);

    let cx = result
        .counterexample()
        .unwrap_or_else(|| panic!("expected a counterexample, got {result:?}"));
    assert_eq!(
        cx.visible_depth(),
        2,
        "the minimal disagreeing visible trace has exactly two observations"
    );
    assert_eq!(cx.kind, MismatchKind::ObservationMismatch);
    assert_eq!(cx.implementation_summand, Summand::Realizing);
    assert_eq!(cx.specification_summand, Summand::Realizing);
    assert_eq!(cx.contract, Contract::Bisimilar);
    assert_eq!(cx.implementation, PresentationIdV1::from_canon(b"left@1"));
    assert_eq!(cx.specification, PresentationIdV1::from_canon(b"right@1"));

    // Identity is a function of the disagreement, not of how it was found.
    assert_eq!(cx.digest(), cx.clone().digest());
}

/// The divergence-sensitivity clause at the comparison level: a terminal state
/// and an infinitely-searching state are not bisimilar, and the mismatch is
/// reported as its own kind rather than folded into a generic summand
/// disagreement.
/// **The direction fixture**, and the divergence-sensitivity clause, on one
/// pair.
///
/// The specification diverges where the implementation terminates. Under
/// `Refines` the specification is underspecified there and imposes no
/// obligation, so the contract holds; under `Bisimilar` the *same two systems*
/// fail with [`MismatchKind::DivergenceVsQuiescence`]. Running both contracts
/// over one pair is the point: if they agreed here, one of them would be
/// redundant.
#[test]
fn a_terminating_impl_refines_a_diverging_spec_but_is_not_bisimilar_to_it() {
    let terminating = build_fixture(&[("w0", "w1", vec![gen_tau()])]);
    let looping = build_fixture(&[("w0", "w1", vec![gen_tau()]), ("w1", "w0", vec![gen_tau()])]);
    let profile = hiding_profile();

    let terminating_regime: &dyn SettlementRegime = &terminating.regime;
    let terminating_regimes = std::slice::from_ref(&terminating_regime);
    let looping_regime: &dyn SettlementRegime = &looping.regime;
    let looping_regimes = std::slice::from_ref(&looping_regime);

    let mut implementation = PresentedSystem::new(
        presentation(
            b"terminating@1",
            terminating_regimes,
            &profile,
            &terminating.interner,
            DeclaredAssumptions::all(),
        ),
        terminating.exec_at("w0"),
        keyer(),
        budget(),
    );
    let mut specification = PresentedSystem::new(
        presentation(
            b"looping@1",
            looping_regimes,
            &profile,
            &looping.interner,
            DeclaredAssumptions::all(),
        ),
        looping.exec_at("w0"),
        keyer(),
        budget(),
    );

    let refines = check_saturated(
        &mut implementation,
        &mut specification,
        Contract::Refines,
        64,
    );
    assert!(
        refines.holds(),
        "replacing a loop with a stop is legal, got {refines:?}"
    );

    let bisimilar = check_saturated(
        &mut implementation,
        &mut specification,
        Contract::Bisimilar,
        64,
    );
    let cx = bisimilar
        .counterexample()
        .unwrap_or_else(|| panic!("the same pair must fail Bisimilar, got {bisimilar:?}"));
    assert_eq!(cx.kind, MismatchKind::DivergenceVsQuiescence);
    assert_eq!(cx.visible_depth(), 0, "they part company immediately");
    assert_eq!(cx.implementation_summand, Summand::Quiescent);
    assert_eq!(cx.specification_summand, Summand::Divergent);
    assert_eq!(cx.contract, Contract::Bisimilar);
}

/// The forbidden direction. The implementation spins where the specification
/// stops, and `Refines` must reject it — otherwise the asymmetry would be a
/// blanket exemption rather than a direction.
#[test]
fn refines_rejects_an_implementation_that_spins_where_the_spec_stops() {
    // Roles swapped relative to `divergence_pair`: the *implementation* loops.
    let terminating = build_fixture(&[("w0", "w1", vec![gen_tau()])]);
    let looping = build_fixture(&[("w0", "w1", vec![gen_tau()]), ("w1", "w0", vec![gen_tau()])]);
    let profile = hiding_profile();

    let looping_regime: &dyn SettlementRegime = &looping.regime;
    let looping_regimes = std::slice::from_ref(&looping_regime);
    let terminating_regime: &dyn SettlementRegime = &terminating.regime;
    let terminating_regimes = std::slice::from_ref(&terminating_regime);

    let mut implementation = PresentedSystem::new(
        presentation(
            b"looping@1",
            looping_regimes,
            &profile,
            &looping.interner,
            DeclaredAssumptions::all(),
        ),
        looping.exec_at("w0"),
        keyer(),
        budget(),
    );
    let mut specification = PresentedSystem::new(
        presentation(
            b"terminating@1",
            terminating_regimes,
            &profile,
            &terminating.interner,
            DeclaredAssumptions::all(),
        ),
        terminating.exec_at("w0"),
        keyer(),
        budget(),
    );

    let result = check_saturated(
        &mut implementation,
        &mut specification,
        Contract::Refines,
        64,
    );
    let cx = result
        .counterexample()
        .unwrap_or_else(|| panic!("expected a counterexample, got {result:?}"));
    assert_eq!(cx.kind, MismatchKind::DivergenceVsQuiescence);
    assert_eq!(cx.implementation_summand, Summand::Divergent);
    assert_eq!(cx.specification_summand, Summand::Quiescent);
}

// ---------------------------------------------------------------------------
// Fail-closed preconditions.
// ---------------------------------------------------------------------------

#[test]
fn a_system_that_does_not_declare_p1_makes_the_comparison_unknown() {
    let a = mixed_fixture();
    let b = mixed_fixture();
    let profile = hiding_profile();
    let a_regime: &dyn SettlementRegime = &a.regime;
    let a_regimes = std::slice::from_ref(&a_regime);
    let b_regime: &dyn SettlementRegime = &b.regime;
    let b_regimes = std::slice::from_ref(&b_regime);

    let mut left = PresentedSystem::new(
        presentation(
            b"a@1",
            a_regimes,
            &profile,
            &a.interner,
            DeclaredAssumptions {
                history_independent: false,
                phase_stable_keying: true,
            },
        ),
        a.exec_at("w0"),
        keyer(),
        budget(),
    );
    let mut right = PresentedSystem::new(
        presentation(
            b"b@1",
            b_regimes,
            &profile,
            &b.interner,
            DeclaredAssumptions::all(),
        ),
        b.exec_at("w0"),
        keyer(),
        budget(),
    );

    assert_eq!(
        check_saturated(&mut left, &mut right, Contract::Bisimilar, 64),
        SaturatedComparison::Unknown(ComparisonUnknown::UndeclaredAssumption(
            AssumptionId::HistoryIndependence
        )),
        "the coinductive close is unsound without P1, so there is no verdict"
    );
}

#[test]
fn systems_at_different_observation_boundaries_are_not_comparable() {
    let a = mixed_fixture();
    let b = mixed_fixture();
    let hiding = hiding_profile();
    // A profile that hides nothing — a different observation boundary entirely.
    let mut all = BTreeSet::from([gen_tau()]);
    all.extend(["a", "b", "c", "d"].into_iter().map(gen_o));
    let exposing = GeneratorPartitionProfile::all_realizing(all);

    let a_regime: &dyn SettlementRegime = &a.regime;
    let a_regimes = std::slice::from_ref(&a_regime);
    let b_regime: &dyn SettlementRegime = &b.regime;
    let b_regimes = std::slice::from_ref(&b_regime);

    let mut left = PresentedSystem::new(
        presentation(
            b"a@1",
            a_regimes,
            &hiding,
            &a.interner,
            DeclaredAssumptions::all(),
        ),
        a.exec_at("w0"),
        keyer(),
        budget(),
    );
    let mut right = PresentedSystem::new(
        presentation(
            b"b@1",
            b_regimes,
            &exposing,
            &b.interner,
            DeclaredAssumptions::all(),
        ),
        b.exec_at("w0"),
        keyer(),
        budget(),
    );

    assert_eq!(
        check_saturated(&mut left, &mut right, Contract::Bisimilar, 64),
        SaturatedComparison::Unknown(ComparisonUnknown::BoundaryMismatch),
        "\"same behavior\" has no meaning across two observation boundaries"
    );
}

#[test]
fn a_step_that_establishes_nothing_makes_the_comparison_unknown() {
    let a = mixed_fixture();
    let b = mixed_fixture();
    let profile = hiding_profile();
    let a_regime: &dyn SettlementRegime = &a.regime;
    let a_regimes = std::slice::from_ref(&a_regime);
    let b_regime: &dyn SettlementRegime = &b.regime;
    let b_regimes = std::slice::from_ref(&b_regime);

    // A hidden-step budget of zero: the very first τ step exhausts it.
    let starved = SaturationBudget {
        max_hidden_steps: 0,
        max_administrative_states: 64,
        max_visible_steps: 64,
    };
    let mut left = PresentedSystem::new(
        presentation(
            b"a@1",
            a_regimes,
            &profile,
            &a.interner,
            DeclaredAssumptions::all(),
        ),
        a.exec_at("w0"),
        keyer(),
        starved,
    );
    let mut right = PresentedSystem::new(
        presentation(
            b"b@1",
            b_regimes,
            &profile,
            &b.interner,
            DeclaredAssumptions::all(),
        ),
        b.exec_at("w0"),
        keyer(),
        budget(),
    );

    match check_saturated(&mut left, &mut right, Contract::Bisimilar, 64) {
        SaturatedComparison::Unknown(ComparisonUnknown::ImplementationUnknown {
            visible_depth,
            ..
        }) => assert_eq!(visible_depth, 0),
        other => panic!("a starved side must be Unknown, never a verdict: {other:?}"),
    }
}
