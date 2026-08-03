//! Stage A gate for divergence-sensitive saturation (ADR-0014 §9, #61).
//!
//! Covers #61's acceptance criteria 1 (a finite `τ*;o` prefix produces the same
//! visible realizing step as its saturated form) and 4 (saturated stepping is
//! deterministic under the frozen calendar key and observation profile), plus
//! the ⟨D-TAU⟩ pin that τ-ness is a *declared profile projection* rather than a
//! property of the step, and the `run`/`run_reason` fix for the
//! quiesced-versus-exhausted conflation.
//!
//! Stage A deliberately does **not** certify divergence: an unbounded
//! administrative orbit exhausts its budget and is reported as an explicit
//! `Unknown`, never as quiescence. The lasso certificate arrives with Stage B.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonWriter, Digest, Domain};
use brix_semantic::{ConfigId, ContextId, Decomposition, GeneratorId};
use soc_core::adm::AdmAll;
use soc_core::calendar::Key;
use soc_core::commit::{commit_tick, run_reason, Committed, SettlementRegime, UnsaturatedStop};
use soc_core::exec::ExecConfig;
use soc_core::history::History;
use soc_core::intern::{Handle, Interner};
use soc_core::journal::Journal;
use soc_core::regime::{Candidate, Regime};
use soc_core::saturate::{
    sat_step, DeclaredAssumptions, GeneratorPartitionProfile, ObservationProfile, PresentationIdV1,
    PresentationV1, ProfileError, SaturatedStep, SaturationBudget, SaturationUnknown, StepLabel,
};

// ---------------------------------------------------------------------------
// Fixture: a world graph whose every edge names the generator that realizes it,
// so an observation profile can partition the graph into administrative and
// realizing regions. Follows the `FixtureRegime` edge-table idiom already used
// by `governance_conservation.rs` and `calendar_commit.rs`.
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
        let src = self.configs[&e.world];
        let dst = self.configs[&c.successor];
        // `configs.len() == generators.len() + 1` — chain the endpoints through
        // one intermediate per extra generator so multi-generator edges are
        // well-formed.
        let mut configs = vec![src];
        for _ in 1..edge.generators.len() {
            configs.push(src);
        }
        configs.push(dst);
        Decomposition::recorded(edge.generators.clone(), configs).expect("well-formed chain")
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

fn gen_tau_a() -> GeneratorId {
    GeneratorId::named("saturation-fixture.tau.a@1")
}
fn gen_tau_b() -> GeneratorId {
    GeneratorId::named("saturation-fixture.tau.b@1")
}
fn gen_realizing() -> GeneratorId {
    GeneratorId::named("saturation-fixture.realizing@1")
}

fn admin_partition() -> BTreeSet<GeneratorId> {
    [gen_tau_a(), gen_tau_b()].into_iter().collect()
}
fn realizing_partition() -> BTreeSet<GeneratorId> {
    [gen_realizing()].into_iter().collect()
}

/// The v1 profile that hides the two τ generators.
fn hiding_profile() -> GeneratorPartitionProfile {
    GeneratorPartitionProfile::new(admin_partition(), realizing_partition())
        .expect("disjoint partitions")
}

/// A profile that hides nothing — every generator is realizing.
fn all_realizing_profile() -> GeneratorPartitionProfile {
    let mut all = admin_partition();
    all.extend(realizing_partition());
    GeneratorPartitionProfile::all_realizing(all)
}

struct Fixture {
    interner: Interner,
    regime: ChainRegime,
    worlds: BTreeMap<&'static str, Handle>,
    policy: Handle,
}

/// `w0 -τa-> w1 -τb-> w2 -o-> w3`, with `w3` terminal.
fn linear_fixture() -> Fixture {
    build_fixture(&[
        ("w0", "w1", vec![gen_tau_a()]),
        ("w1", "w2", vec![gen_tau_b()]),
        ("w2", "w3", vec![gen_realizing()]),
    ])
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
    let policy = tag(&mut interner, "policy");
    let regime_id = tag(&mut interner, "regime.chain");

    let mut configs = BTreeMap::new();
    for (_, handle) in worlds.iter() {
        configs.insert(*handle, ConfigId(interner.resolve(*handle)));
    }

    let mut edges = BTreeMap::new();
    for (from, to, generators) in spec {
        let from_h = worlds[from];
        let to_h = worlds[to];
        let witness = tag(&mut interner, &format!("witness.{from}->{to}"));
        edges.insert(
            from_h,
            Edge {
                witness,
                successor: to_h,
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

/// Assemble a presentation. The regime slice must be a caller-held local — a
/// `&[&dyn SettlementRegime]` cannot outlive the `&dyn` temporary it borrows.
fn presentation<'a>(
    regimes: &'a [&'a dyn SettlementRegime],
    profile: &'a dyn ObservationProfile,
    interner: &'a Interner,
) -> PresentationV1<'a> {
    PresentationV1 {
        id: PresentationIdV1::from_canon(b"saturation-fixture@1"),
        regimes,
        regime_set: Digest::of(Domain::Value, b"saturation-fixture.regime-set"),
        adm: &AdmAll,
        adm_id: Digest::of(Domain::Value, b"saturation-fixture.adm-all"),
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

// ---------------------------------------------------------------------------
// AC-1 — a finite `τ*;o` prefix produces the same visible realizing step as its
// saturated form.
// ---------------------------------------------------------------------------

#[test]
fn finite_tau_prefix_exports_the_same_observation_as_the_unsaturated_step() {
    let fx = linear_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner);
    let mut k = keyer();
    let (saturated, consumed, cost) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());

    // The unsaturated step taken directly from w2, where the realizing edge is.
    let mut k2 = keyer();
    let (direct, _, _) = commit_tick(
        regimes,
        &AdmAll,
        &fx.interner,
        &fx.exec_at("w2"),
        ContextId::root(),
        0,
        &mut k2,
    );

    match (saturated, direct) {
        (
            SaturatedStep::Realizing {
                observation,
                hidden_steps,
                ..
            },
            Committed::Step {
                observation: direct_observation,
                ..
            },
        ) => {
            assert_eq!(
                observation, direct_observation,
                "saturating a finite τ-prefix must export the same O_min value"
            );
            assert_eq!(hidden_steps, 2, "two administrative steps were hidden");
        }
        other => panic!("expected Realizing on both sides, got {other:?}"),
    }

    // All three steps are committed and journalable — hiding is a visibility
    // projection, not an erasure (⟨D-TAU⟩).
    assert_eq!(consumed.len(), 3, "τ steps are committed, not dropped");
    assert!(
        cost.work_units().is_some(),
        "cost is never silently unmeasured"
    );
}

// ---------------------------------------------------------------------------
// AC-4 — determinism under the frozen calendar key and observation profile.
// ---------------------------------------------------------------------------

#[test]
fn saturated_stepping_is_deterministic_across_two_runs() {
    let fx = linear_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);

    let run_once = || {
        let pres = presentation(regimes, &profile, &fx.interner);
        let mut k = keyer();
        let (step, consumed, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());
        (step, Journal::replay_chain(&consumed))
    };

    let (first_step, first_chain) = run_once();
    let (second_step, second_chain) = run_once();

    assert_eq!(first_step, second_step, "saturated step must be stable");
    assert_eq!(
        first_chain, second_chain,
        "the committed chain under saturation must be byte-identical across runs"
    );
}

// ---------------------------------------------------------------------------
// ⟨D-TAU⟩ — τ-ness is declared, not intrinsic.
// ---------------------------------------------------------------------------

#[test]
fn one_journal_under_two_profiles_has_two_visible_traces_and_one_chain_digest() {
    let fx = linear_fixture();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let mut k = keyer();

    // Drive the raw committed loop to build ONE journal, profile-independently.
    let (journal, _, stop) = run_reason(
        regimes,
        &AdmAll,
        &fx.interner,
        fx.exec_at("w0"),
        ContextId::root(),
        &mut k,
        16,
    );
    assert_eq!(stop, UnsaturatedStop::ImmediateFrontierEmpty);
    assert_eq!(journal.len(), 3);

    let visible = |profile: &dyn ObservationProfile| -> Vec<StepLabel> {
        journal
            .steps()
            .iter()
            .map(|s| profile.label(s).expect("fixture generators are registered"))
            .collect()
    };

    let hiding = hiding_profile();
    let all_realizing = all_realizing_profile();

    assert_eq!(
        visible(&hiding),
        vec![
            StepLabel::Administrative,
            StepLabel::Administrative,
            StepLabel::Realizing
        ]
    );
    assert_eq!(
        visible(&all_realizing),
        vec![StepLabel::Realizing; 3],
        "a profile that hides nothing sees every step"
    );
    assert_ne!(
        hiding.id(),
        all_realizing.id(),
        "distinct partitions must have distinct canonical identities"
    );

    // One committed chain either way — the journal does not know about profiles.
    assert_eq!(
        journal.chain_digest(),
        Journal::replay_chain(journal.steps())
            .last()
            .copied()
            .expect("three steps were committed"),
        "one committed chain, independent of any observation profile"
    );
}

#[test]
fn an_all_realizing_profile_makes_saturation_degenerate_to_one_committed_step() {
    let fx = linear_fixture();
    let profile = all_realizing_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner);
    let mut k = keyer();
    let (step, consumed, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());

    match step {
        SaturatedStep::Realizing { hidden_steps, .. } => assert_eq!(hidden_steps, 0),
        other => panic!("expected Realizing, got {other:?}"),
    }
    assert_eq!(
        consumed.len(),
        1,
        "with nothing hidden, sat_step consumes exactly one γ-tick"
    );
}

// ---------------------------------------------------------------------------
// Fail-closed profile classification.
// ---------------------------------------------------------------------------

#[test]
fn a_mixed_generator_decomposition_fails_closed_without_an_observation() {
    // One edge whose decomposition draws from BOTH partitions.
    let fx = build_fixture(&[("w0", "w1", vec![gen_tau_a(), gen_realizing()])]);
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner);
    let mut k = keyer();
    let (step, _, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());

    assert_eq!(
        step,
        SaturatedStep::Unknown(SaturationUnknown::ProfileError {
            at_step: 0,
            error: ProfileError::MixedDecomposition,
        }),
        "a step that is neither wholly τ nor wholly realizing must fail closed"
    );
}

#[test]
fn an_unregistered_generator_fails_closed() {
    let fx = build_fixture(&[(
        "w0",
        "w1",
        vec![GeneratorId::named("saturation-fixture.unknown@1")],
    )]);
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner);
    let mut k = keyer();
    let (step, _, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());

    assert_eq!(
        step,
        SaturatedStep::Unknown(SaturationUnknown::ProfileError {
            at_step: 0,
            error: ProfileError::UnregisteredGenerator,
        }),
        "a generator in neither partition must fail closed, not default"
    );
}

// ---------------------------------------------------------------------------
// Quiescence, and the honest refusal to certify an unbounded τ-orbit.
// ---------------------------------------------------------------------------

#[test]
fn a_terminal_configuration_yields_a_quiescence_claim_graded_derived() {
    let fx = linear_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner);
    let mut k = keyer();
    let (step, consumed, _) = sat_step(&pres, &fx.exec_at("w3"), 0, &mut k, budget());

    match step {
        SaturatedStep::Quiescent(cert) => {
            assert_eq!(cert.grade, brix_semantic::Outcome::Derived);
            assert_eq!(cert.src_world, cert.terminal_world, "no τ prefix at w3");
            assert!(cert.hidden.is_empty());
            assert_eq!(
                cert.prefix_chain,
                History::empty().digest(),
                "an empty prefix chains to the empty history, not to a sentinel"
            );
            assert_eq!(cert.profile, profile.id());
        }
        other => panic!("expected Quiescent at the terminal world, got {other:?}"),
    }
    assert!(consumed.is_empty());
}

#[test]
fn a_tau_prefix_before_quiescence_is_recorded_in_the_claim() {
    // w0 -τa-> w1, and w1 is terminal: saturation hides one step, then certifies.
    let fx = build_fixture(&[("w0", "w1", vec![gen_tau_a()])]);
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner);
    let mut k = keyer();
    let (step, consumed, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());

    match step {
        SaturatedStep::Quiescent(cert) => {
            assert_eq!(cert.hidden.len(), 1, "the τ step is recorded, not erased");
            assert_ne!(
                cert.src_world, cert.terminal_world,
                "saturation advanced the world before quiescing"
            );
            assert_ne!(
                cert.prefix_chain,
                History::empty().digest(),
                "a non-empty prefix must chain past the empty history"
            );
        }
        other => panic!("expected Quiescent after a finite τ prefix, got {other:?}"),
    }
    assert_eq!(consumed.len(), 1, "the hidden step is still committed");
}

/// A τ-chain longer than the budget. Note this is a chain, not a loop: it never
/// revisits a state, so Stage B's lasso detection has nothing to find and the
/// only honest answer is exhaustion. The self-loop case — which *is* a lasso and
/// so is certified rather than bounded — lives in `saturation_certificates.rs`.
#[test]
fn a_tau_chain_past_the_budget_exhausts_and_never_certifies_quiescence() {
    let fx = build_fixture(&[
        ("w0", "w1", vec![gen_tau_a()]),
        ("w1", "w2", vec![gen_tau_a()]),
        ("w2", "w3", vec![gen_tau_a()]),
        ("w3", "w4", vec![gen_tau_a()]),
        ("w4", "w5", vec![gen_tau_a()]),
        ("w5", "w6", vec![gen_tau_a()]),
        ("w6", "w7", vec![gen_tau_a()]),
        ("w7", "w8", vec![gen_tau_a()]),
        ("w8", "w9", vec![gen_tau_a()]),
        ("w9", "w10", vec![gen_realizing()]),
    ]);
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner);
    let mut k = keyer();
    let (step, _, _) = sat_step(
        &pres,
        &fx.exec_at("w0"),
        0,
        &mut k,
        SaturationBudget {
            max_hidden_steps: 3,
            max_administrative_states: 64,
            max_visible_steps: 64,
        },
    );

    match step {
        SaturatedStep::Unknown(SaturationUnknown::AdministrativeBudgetExhausted {
            hidden_steps,
            budget,
        }) => {
            assert_eq!(budget, 3);
            assert_eq!(hidden_steps, 4, "the bound is hit one step past the budget");
        }
        other => panic!("a τ-chain past its budget must be Unknown, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The headline defect: `run` could not distinguish quiesced from exhausted.
// ---------------------------------------------------------------------------

#[test]
fn run_reason_distinguishes_an_empty_frontier_from_an_exhausted_tick_budget() {
    let terminating = linear_fixture();
    let regime: &dyn SettlementRegime = &terminating.regime;
    let regimes = std::slice::from_ref(&regime);
    let mut k = keyer();
    let (journal, _, stop) = run_reason(
        regimes,
        &AdmAll,
        &terminating.interner,
        terminating.exec_at("w0"),
        ContextId::root(),
        &mut k,
        16,
    );
    assert_eq!(stop, UnsaturatedStop::ImmediateFrontierEmpty);
    assert_eq!(journal.len(), 3);

    // The same driver on a non-terminating world, cut off by its tick budget.
    let looping = build_fixture(&[("w0", "w0", vec![gen_tau_a()])]);
    let looping_regime: &dyn SettlementRegime = &looping.regime;
    let looping_regimes = std::slice::from_ref(&looping_regime);
    let mut k2 = keyer();
    let (looping_journal, _, looping_stop) = run_reason(
        looping_regimes,
        &AdmAll,
        &looping.interner,
        looping.exec_at("w0"),
        ContextId::root(),
        &mut k2,
        4,
    );

    assert_eq!(
        looping_stop,
        UnsaturatedStop::TickBudgetExhausted { max_ticks: 4 },
        "running out of ticks must be distinguishable from an empty frontier"
    );
    assert_eq!(looping_journal.len(), 4);
    assert_ne!(
        stop, looping_stop,
        "the two stop reasons are exactly what `run` alone conflated"
    );
}

// ---------------------------------------------------------------------------
// ⟨D-TAU⟩'s load-bearing claim, made executable.
// ---------------------------------------------------------------------------

/// Every semantics this fixture needs: an edge's generator realizes exactly
/// that edge's endpoints.
struct EdgeSemantics {
    edges: BTreeSet<(GeneratorId, ConfigId, ConfigId)>,
}

impl soc_core::audit::GeneratorSemantics for EdgeSemantics {
    fn realizes(&self, g: &GeneratorId, src: &ConfigId, dst: &ConfigId) -> bool {
        self.edges.contains(&(*g, *src, *dst))
    }
}

/// **The claim ⟨D-TAU⟩ rests on**: an administrative step is committed,
/// journaled, `Derived`, and *auditable* — hidden at the observation boundary
/// and in no other way diminished.
///
/// ADR-0014 §3 forbids the "τ as a committed step with its `Observation`
/// suppressed" alternative precisely because it would break `audit_step`,
/// which requires `step.observation.judgement_digest` to match the `Derived`
/// judgement it reconstructs. That argument is stated in several places in
/// prose; this is the executable form of it. If a τ step ever failed to audit,
/// the reason τ is a profile projection rather than a new summand would be
/// false, and nothing else in the suite would notice.
#[test]
fn administrative_steps_audit_exactly_like_realizing_ones() {
    let fx = linear_fixture(); // w0 -τa-> w1 -τb-> w2 -o-> w3
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner);
    let mut k = keyer();

    let (step, consumed, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());
    assert!(matches!(step, SaturatedStep::Realizing { .. }));
    assert_eq!(
        consumed.len(),
        3,
        "two hidden τ steps plus the realizing one"
    );

    // Two of the three are administrative — otherwise this test would be
    // auditing realizing steps only and proving nothing about τ.
    let labels: Vec<StepLabel> = consumed
        .iter()
        .map(|s| profile.label(s).expect("classifiable"))
        .collect();
    assert_eq!(
        labels,
        vec![
            StepLabel::Administrative,
            StepLabel::Administrative,
            StepLabel::Realizing
        ]
    );

    let mut journal = Journal::new();
    for committed in &consumed {
        journal.append(committed.clone());
    }

    let mut registry = brix_semantic::GeneratorRegistry::new();
    for g in [gen_tau_a(), gen_tau_b(), gen_realizing()] {
        registry.insert(g);
    }
    let semantics = EdgeSemantics {
        edges: consumed
            .iter()
            .flat_map(|s| {
                s.decomposition
                    .generators
                    .iter()
                    .map(|g| (*g, s.src, s.dst))
                    .collect::<Vec<_>>()
            })
            .collect(),
    };

    let results =
        soc_core::audit::audit_journal(&journal, ContextId::root(), &registry, &semantics);

    assert_eq!(results.len(), 3);
    for (index, result) in results.iter().enumerate() {
        match result {
            soc_core::audit::AuditResult::Audited(_) => {}
            soc_core::audit::AuditResult::Unknown(reason) => panic!(
                "step {index} ({:?}) failed to audit: {reason} — a hidden step must \
                 still be fully auditable, or ⟨D-TAU⟩'s argument collapses",
                labels[index]
            ),
        }
    }
}
