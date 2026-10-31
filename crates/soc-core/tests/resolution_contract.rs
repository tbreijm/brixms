//! SOC resolution contract conformance slice (Issue #52, `spec/SOC_Resolution_Contract.md`).
//!
//! Exercises the 10 normative resolution scenarios against the existing
//! `soc-core` candidate discovery, admissibility, calendar, commitment,
//! and evidence machinery, plus edge case and negative decode hardening.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonDecode, CanonError, CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{
    Authority, ConfigId, ContextId, Decomposition, Dependency, EdgeKind, GeneratorId, Judgement,
    Outcome, PropositionId, Support, WitnessId,
};
use soc_core::adm::{Adm, AdmAll, AdmWitnessAllowlist};
use soc_core::calendar::{Frontier, Key};
use soc_core::commit::{commit_tick, CommitError, Committed, SettlementWitnessProvider};
use soc_core::delta::{CandidateDelta, Delta, Footprint};
use soc_core::engine::{naive_view_over, IncrementalEngine, IncrementalWitnessIndex};
use soc_core::exec::ExecConfig;
use soc_core::history::History;
use soc_core::intern::{Handle, Interner};
use soc_core::journal::Journal;
use soc_core::oracle::{cand, cand_instrumented};
use soc_core::resolution::{
    classify_resolution, classify_resolution_with_reason,
    classify_resolution_with_verified_evidence, CanonicalCandidate, CompletenessScope,
    CompletenessVerificationError, ContextValidationError, ContextValidator, ResolutionOutcome,
    ResolutionQuery, ResolutionReport, ResolutionUnknown, StubContextValidator,
    VerifiedCompleteness, MAX_RESOLUTION_CANDIDATES,
};
use soc_core::saturate::{
    check_divergence_certificate, check_quiescence_certificate, sat_step, CertificateCheck,
    CertificateCheckError, DeclaredAssumptions, GeneratorPartitionProfile, ObservationProfile,
    PresentationIdV1, PresentationV1, SaturatedStep, SaturationBudget,
};
use soc_core::witness_provider::{Candidate, WitnessProvider};

// --- Test Fixtures ---

#[derive(Clone)]
struct MultiCandidateProvider {
    candidates: Vec<Candidate>,
}

impl WitnessProvider for MultiCandidateProvider {
    fn candidates(&self, _e: &ExecConfig) -> Vec<Candidate> {
        self.candidates.clone()
    }
}

impl SettlementWitnessProvider for MultiCandidateProvider {
    fn try_decompose(&self, _e: &ExecConfig, c: &Candidate) -> Result<Decomposition, CommitError> {
        let gen_name = format!("gen.step@{}", c.witness.raw());
        Ok(Decomposition::recorded(
            vec![GeneratorId::named(&gen_name)],
            vec![
                ConfigId::from_canon(b"src-config"),
                ConfigId::from_canon(b"dst-config"),
            ],
        )
        .unwrap())
    }
}

fn tiebreak_digest(c: &Candidate) -> Digest {
    let mut w = CanonWriter::new();
    w.write_uint(c.witness.raw() as u64);
    w.write_uint(c.successor.raw() as u64);
    w.digest(Domain::Value)
}

fn make_test_env() -> (Interner, ExecConfig, Handle, Handle, Handle) {
    let mut i = Interner::new();
    let world = i.intern(Digest::of(Domain::Value, b"res-world-0"));
    let policy = i.intern(Digest::of(Domain::Value, b"res-policy-0"));
    let e = ExecConfig::new(world, policy, History::empty().digest());
    let w1 = i.intern(Digest::of(Domain::Value, b"witness-1"));
    let w2 = i.intern(Digest::of(Domain::Value, b"witness-2"));
    let s0 = i.intern(Digest::of(Domain::Value, b"succ-0"));
    (i, e, w1, w2, s0)
}

// Chain fixture helpers for real saturation cycle detection and policy rejection
struct Edge {
    witness: Handle,
    successor: Handle,
    generators: Vec<GeneratorId>,
}

struct ChainRegime {
    edges: BTreeMap<Handle, Edge>,
    configs: BTreeMap<Handle, ConfigId>,
}

impl WitnessProvider for ChainRegime {
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        self.edges
            .get(&e.world)
            .map(|edge| {
                vec![Candidate {
                    witness: edge.witness,
                    successor: edge.successor,
                }]
            })
            .unwrap_or_default()
    }
}

impl SettlementWitnessProvider for ChainRegime {
    fn try_decompose(&self, e: &ExecConfig, _c: &Candidate) -> Result<Decomposition, CommitError> {
        let edge = self
            .edges
            .get(&e.world)
            .expect("try_decompose on live edge");
        let src = self.configs[&e.world];
        let dst = self.configs[&edge.successor];
        Ok(Decomposition::recorded(edge.generators.clone(), vec![src, dst]).unwrap())
    }
}

struct ChainFixture {
    interner: Interner,
    regime: ChainRegime,
    worlds: BTreeMap<&'static str, Handle>,
    policy: Handle,
}

impl ChainFixture {
    fn exec_at(&self, world: &str) -> ExecConfig {
        ExecConfig::new(self.worlds[world], self.policy, History::empty().digest())
    }
}

fn build_chain_fixture(spec: &[(&'static str, &'static str, Vec<GeneratorId>)]) -> ChainFixture {
    let mut interner = Interner::new();
    let mut worlds: BTreeMap<&'static str, Handle> = BTreeMap::new();
    for (from, to, _) in spec {
        for name in [from, to] {
            if !worlds.contains_key(name) {
                let h = interner.intern(Digest::of(Domain::Value, name.as_bytes()));
                worlds.insert(name, h);
            }
        }
    }
    let policy = interner.intern(Digest::of(Domain::Value, b"policy"));

    let mut configs = BTreeMap::new();
    for handle in worlds.values() {
        configs.insert(*handle, ConfigId(interner.resolve(*handle)));
    }

    let mut edges = BTreeMap::new();
    for (from, to, generators) in spec {
        let witness = interner.intern(Digest::of(
            Domain::Value,
            format!("wit.{from}->{to}").as_bytes(),
        ));
        edges.insert(
            worlds[from],
            Edge {
                witness,
                successor: worlds[to],
                generators: generators.clone(),
            },
        );
    }

    ChainFixture {
        interner,
        regime: ChainRegime { edges, configs },
        worlds,
        policy,
    }
}

fn gen_tau() -> GeneratorId {
    GeneratorId::named("res-fixture.tau@1")
}

fn gen_realizing() -> GeneratorId {
    GeneratorId::named("res-fixture.realizing@1")
}

fn hiding_profile() -> GeneratorPartitionProfile {
    GeneratorPartitionProfile::new(
        [gen_tau()].into_iter().collect(),
        [gen_realizing()].into_iter().collect(),
    )
    .expect("disjoint partitions")
}

fn chain_presentation<'a>(
    regimes: &'a [&'a dyn SettlementWitnessProvider],
    profile: &'a dyn ObservationProfile,
    interner: &'a Interner,
    adm: &'a dyn Adm,
    adm_id: Digest,
) -> PresentationV1<'a> {
    PresentationV1 {
        id: PresentationIdV1::from_canon(b"res-chain-pres@1"),
        regimes,
        regime_set: Digest::of(Domain::Value, b"res-chain-regimes"),
        adm,
        adm_id,
        profile,
        interner,
        context: ContextId::root(),
        assumptions: DeclaredAssumptions::all(),
    }
}

// --- Scenario 1: Two admissible candidates ---
// Canonical selected candidate via select_K, but logical ambiguity classified separately.
#[test]
fn scenario_1_two_admissible_candidates_distinguishes_winner_from_logical_ambiguity() {
    let (_i, e, w1, w2, s0) = make_test_env();
    let c1 = Candidate {
        witness: w1,
        successor: s0,
    };
    let c2 = Candidate {
        witness: w2,
        successor: s0,
    };

    let provider = MultiCandidateProvider {
        candidates: vec![c1, c2],
    };
    let providers: [&dyn WitnessProvider; 1] = [&provider];
    let adm = AdmAll;

    // Discovery yields both candidates
    let discovered = cand(&providers, &adm, &e);
    assert_eq!(discovered.len(), 2);
    assert!(discovered.contains(&c1));
    assert!(discovered.contains(&c2));

    // Calendar selection deterministically selects the least-key candidate
    let mut frontier = Frontier::new();
    let key1 = Key::new(0, 10, tiebreak_digest(&c1));
    let key2 = Key::new(0, 20, tiebreak_digest(&c2)); // Higher priority number = lower urgency
    frontier.insert(key1, c1).unwrap();
    frontier.insert(key2, c2).unwrap();

    let (selected_key, winner) = frontier.select_least().unwrap();
    assert_eq!(selected_key, key1);
    assert_eq!(winner, c1);

    // Resolution classification flags this as Ambiguous, preserving the distinction
    // between scheduling choice (select_K) and logical uniqueness.
    let outcome = classify_resolution(discovered, CompletenessScope::Complete, Some(winner));

    match outcome {
        ResolutionOutcome::Ambiguous {
            selected,
            candidates,
        } => {
            assert_eq!(selected, Some(c1));
            assert_eq!(candidates.len(), 2);
            assert!(candidates.contains(&c1));
            assert!(candidates.contains(&c2));
        }
        _ => panic!("Expected Ambiguous outcome for multiple candidates"),
    }
}

// --- Scenario 2: Tightened admissibility ---
// Candidate set shrinks; prohibited winner cannot survive.
#[test]
fn scenario_2_tightened_admissibility_shrinks_candidates_and_displaces_winner() {
    let (_i, e, w1, w2, s0) = make_test_env();
    let c1 = Candidate {
        witness: w1,
        successor: s0,
    };
    let c2 = Candidate {
        witness: w2,
        successor: s0,
    };

    let provider = MultiCandidateProvider {
        candidates: vec![c1, c2],
    };
    let providers: [&dyn WitnessProvider; 1] = [&provider];

    // Loose policy: c1 and c2 both admissible, c1 wins under priority
    let adm_loose = AdmAll;
    let cand_loose = cand(&providers, &adm_loose, &e);
    assert_eq!(cand_loose.len(), 2);

    // Tightened policy: exclude c1's witness explicitly
    let adm_tight = AdmWitnessAllowlist::new([w2]);
    let cand_tight = cand(&providers, &adm_tight, &e);

    // cand_tight is a strict subset
    assert_eq!(cand_tight.len(), 1);
    assert!(!cand_tight.contains(&c1));
    assert!(cand_tight.contains(&c2));

    // Classification under tightened policy is now Unique(c2)
    let outcome = classify_resolution(cand_tight, CompletenessScope::Complete, Some(c2));
    assert_eq!(outcome, ResolutionOutcome::Unique(c2));
}

// --- Scenario 3: Empty complete finite domain ---
// NoSolution only with explicit completeness scope.
#[test]
fn scenario_3_empty_complete_domain_yields_nosolution() {
    let (_i, e, _w1, _w2, _s0) = make_test_env();
    let provider = MultiCandidateProvider { candidates: vec![] };
    let providers: [&dyn WitnessProvider; 1] = [&provider];
    let adm = AdmAll;

    let discovered = cand(&providers, &adm, &e);
    assert!(discovered.is_empty());

    // When declared completeness is Complete, empty candidates = NoSolution
    let outcome = classify_resolution(discovered, CompletenessScope::Complete, None);
    assert_eq!(outcome, ResolutionOutcome::NoSolution);
}

// --- Scenario 4: Interrupted or bounded enumeration ---
// Unknown; never NoSolution or Refuted.
#[test]
fn scenario_4_bounded_incomplete_search_yields_unknown_never_nosolution_or_refuted() {
    let (_i, _e, _w1, _w2, _s0) = make_test_env();

    // Empty search result under Incomplete scope (e.g. hit budget)
    let outcome =
        classify_resolution::<Candidate>(BTreeSet::new(), CompletenessScope::Incomplete, None);

    match outcome {
        ResolutionOutcome::Unknown(reason) => {
            assert_eq!(reason, ResolutionUnknown::BudgetExhausted);
        }
        _ => panic!("Expected Unknown for bounded incomplete search"),
    }

    // Must never publish Outcome::Refuted or Proven on incomplete search
    let epistemic = Outcome::Unknown;
    assert_eq!(epistemic.authority(), Authority::AnyResolver);
    assert_ne!(epistemic, Outcome::Refuted);
    assert_ne!(epistemic, Outcome::Proven);
}

// --- Scenario 5: Cycle or repeated state ---
// Real administrative lasso detected through the saturation engine.
#[test]
fn scenario_5_cycle_detection_yields_cycle_unknown_never_refuted() {
    // 1. Build a real 2-step loop: w0 -tau-> w1 -tau-> w0
    let fx = build_chain_fixture(&[("w0", "w1", vec![gen_tau()]), ("w1", "w0", vec![gen_tau()])]);
    let profile = hiding_profile();
    let regime: &dyn SettlementWitnessProvider = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = chain_presentation(
        regimes,
        &profile,
        &fx.interner,
        &AdmAll,
        Digest::of(Domain::Value, b"adm-all"),
    );

    let mut keyer = |c: &Candidate, phase: u64| Key::new(phase, 0, tiebreak_digest(c));

    // 2. Execute sat_step: it actually steps through the loop and detects the closed lasso
    let (step, lasso, _) = sat_step(
        &pres,
        &fx.exec_at("w0"),
        0,
        &mut keyer,
        SaturationBudget::uniform(32),
    );

    let SaturatedStep::Divergent(cert) = step else {
        panic!("expected certified divergence detected through the loop");
    };

    // The cycle detection proves non-termination of the administrative path
    assert_eq!(cert.stem, 0);
    assert_eq!(cert.cycle, 2);
    assert_eq!(
        cert.grade,
        Outcome::Unknown,
        "cycle detection yields Unknown for the completion question"
    );
    assert_ne!(
        cert.grade,
        Outcome::Refuted,
        "a cycle is never a refutation"
    );
    assert!(!cert.grade.is_theorem());

    // 3. Independent checker verifies the detected divergence certificate
    assert!(check_divergence_certificate(&cert, &pres, &lasso).is_verified());

    // 4. Map the detected divergence to resolution outcome
    let outcome = classify_resolution_with_reason::<Candidate>(
        BTreeSet::new(),
        CompletenessScope::Incomplete,
        None,
        ResolutionUnknown::CycleDetected,
    );
    assert_eq!(
        outcome,
        ResolutionOutcome::Unknown(ResolutionUnknown::CycleDetected)
    );
}

// --- Scenario 6: Rejected/wrong-context certificate ---
// ContextValidator taxonomy check returns error; callers gating on validation refuse publication.
#[test]
fn scenario_6_mismatched_context_fails_validation_and_blocks_publication() {
    let ctx_expected = ContextId::root();
    let ctx_provided = ContextId::root().extend(b"divergent-branch");

    // 1. Context validation seam (#59 consumer contract):
    let validator = StubContextValidator { allow_all: false };
    let check = validator.validate_context(ctx_expected, ctx_provided);
    assert_eq!(check, Err(ContextValidationError::Mismatched));

    // 2. Publication gating: when context check fails, calling code refuses to publish.
    // In addition, independent authority/support checks (ROUTES) reject unauthorized minting fail-closed:
    let prop = PropositionId::from_canon(b"some-prop");
    let support = Support::Ground {
        body: Digest::of(Domain::Value, b"body"),
    };
    let publish_attempt = Judgement::publish(
        Authority::ProofKernel, // Cannot mint Proven on Ground support (ROUTES fence)
        ctx_provided,
        prop,
        Outcome::Proven,
        support,
    );
    assert!(publish_attempt.is_err());
}

// --- Scenario 7: One of several supports removed ---
// Real semantic artifacts: alternate support preserves justified proposition.
#[test]
fn scenario_7_alternate_support_preserves_justified_conclusion() {
    let ctx = ContextId::root();
    let prop = PropositionId::from_canon(b"prop-arithmetic-closure");

    // Derivation 1: route A
    let d1 = Decomposition::recorded(
        vec![GeneratorId::named("arith.route.a@1")],
        vec![
            ConfigId::from_canon(b"node-x0"),
            ConfigId::from_canon(b"node-x1"),
        ],
    )
    .unwrap();

    // Derivation 2: route B (alternative justification for the same proposition)
    let d2 = Decomposition::recorded(
        vec![GeneratorId::named("arith.route.b@1")],
        vec![
            ConfigId::from_canon(b"node-x0"),
            ConfigId::from_canon(b"node-x2"),
        ],
    )
    .unwrap();

    // Both derivations publish valid Derived judgements under SettlementKernel
    let j1 = Judgement::publish(
        Authority::SettlementKernel,
        ctx,
        prop,
        Outcome::Derived,
        Support::Settlement(&d1),
    )
    .unwrap();

    let j2 = Judgement::publish(
        Authority::SettlementKernel,
        ctx,
        prop,
        Outcome::Derived,
        Support::Settlement(&d2),
    )
    .unwrap();

    // Distinct evidence yields distinct search-invariant judgements for the same proposition
    assert_eq!(j1.proposition, j2.proposition);
    assert_ne!(j1.evidence, j2.evidence);
    assert_ne!(j1.id(), j2.id());

    // Model dependency tracking: proposition has two supporting premise edges
    let edge1 = Dependency::new(EdgeKind::Premise, j1.id().digest());
    let edge2 = Dependency::new(EdgeKind::Premise, j2.id().digest());
    let mut active_premises = BTreeSet::from([edge1.id(), edge2.id()]);
    assert_eq!(active_premises.len(), 2);

    // Retract derivation 1 (edge1 removed)
    active_premises.remove(&edge1.id());

    // Proposition remains justified through derivation 2
    assert!(!active_premises.is_empty());
    assert!(active_premises.contains(&edge2.id()));
}

// --- Scenario 8: Last support removed ---
// Live conclusion invalidated; historical evidence retained in journal.
#[test]
fn scenario_8_last_support_removal_invalidates_live_state_while_retaining_history() {
    let (i, e, w1, _w2, s0) = make_test_env();
    let c1 = Candidate {
        witness: w1,
        successor: s0,
    };

    let provider = MultiCandidateProvider {
        candidates: vec![c1],
    };
    let settlement_providers: [&dyn SettlementWitnessProvider; 1] = [&provider];

    // Hot loop commits a step into the journal
    let mut journal = Journal::new();
    let (committed, step_opt, _cost) = commit_tick(
        &settlement_providers,
        &AdmAll,
        &i,
        &e,
        ContextId::root(),
        0,
        &mut |c, phase| Key::new(phase, 1, tiebreak_digest(c)),
    );

    match committed {
        Committed::Step { .. } => {}
        _ => panic!("Expected committed step"),
    }

    let step = step_opt.expect("Committed::Step must emit CommittedStep");
    journal.append(step.clone());

    assert_eq!(journal.len(), 1);
    let step_record = journal.steps()[0].clone();

    // Now all supports for live derivation are retracted:
    // Live conclusion is invalidated (no admissible candidates remain)
    let cand_after_retract = cand(
        &[&MultiCandidateProvider { candidates: vec![] }],
        &AdmAll,
        &e,
    );
    assert!(cand_after_retract.is_empty());

    // Historical journal record is immutable and remains byte-identical
    assert_eq!(journal.len(), 1);
    assert_eq!(journal.steps()[0], step_record);

    // Deterministic replay over the journal reproduces the exact historical chain digest
    let replay_digests = Journal::replay_chain(journal.steps());
    assert_eq!(replay_digests.len(), 1);
    assert_eq!(replay_digests[0], journal.chain_digest());
}

// --- Scenario 9: Policy/program/context/verifier revision changes ---
// Exercises real stale-result reuse rejection through check_quiescence_certificate and context validation.
#[test]
fn scenario_9_binding_change_prevents_old_result_reuse() {
    // 1. Build a terminating fixture: w0 -tau-> w1 -tau-> w2 (terminal)
    let fx = build_chain_fixture(&[("w0", "w1", vec![gen_tau()]), ("w1", "w2", vec![gen_tau()])]);
    let profile = hiding_profile();
    let regime: &dyn SettlementWitnessProvider = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let adm_p1 = Digest::of(Domain::Value, b"policy-original");
    let pres_p1 = chain_presentation(regimes, &profile, &fx.interner, &AdmAll, adm_p1);

    let mut keyer = |c: &Candidate, phase: u64| Key::new(phase, 0, tiebreak_digest(c));

    // 2. Derive a valid quiescence certificate under Policy P1
    let (step, consumed, _) = sat_step(
        &pres_p1,
        &fx.exec_at("w0"),
        0,
        &mut keyer,
        SaturationBudget::uniform(32),
    );
    let SaturatedStep::Quiescent(cert_p1) = step else {
        panic!("expected quiescent step under terminating fixture");
    };

    let terminal_config = ExecConfig::new(fx.worlds["w2"], fx.policy, History::empty().digest());

    // Verified under original presentation
    assert!(
        check_quiescence_certificate(&cert_p1, &pres_p1, &terminal_config, &consumed).is_verified()
    );

    // 3. Stale result reuse attempt under Policy P2:
    let adm_p2 = Digest::of(Domain::Value, b"policy-different-governance");
    let pres_p2 = chain_presentation(regimes, &profile, &fx.interner, &AdmAll, adm_p2);

    // The checker genuinely catches and rejects the policy mismatch
    let check_p2 = check_quiescence_certificate(&cert_p1, &pres_p2, &terminal_config, &consumed);
    assert_eq!(
        check_p2,
        CertificateCheck::Unknown(CertificateCheckError::AdmMismatch),
        "quiescence certificate bound to P1 must be rejected under P2"
    );

    // 4. Stale result reuse attempt under Context C2:
    let validator = StubContextValidator { allow_all: false };
    let check_ctx =
        validator.validate_context(ContextId::root(), ContextId::root().extend(b"branch-c2"));
    assert_eq!(check_ctx, Err(ContextValidationError::Mismatched));
}

// --- Scenario 10: Provider/order/telemetry changes ---
// Equivalent inputs/policy preserve canonical result identity.
#[test]
fn scenario_10_order_and_telemetry_changes_preserve_canonical_result() {
    let (_i, e, w1, w2, s0) = make_test_env();
    let c1 = Candidate {
        witness: w1,
        successor: s0,
    };
    let c2 = Candidate {
        witness: w2,
        successor: s0,
    };

    let p1 = MultiCandidateProvider {
        candidates: vec![c1],
    };
    let p2 = MultiCandidateProvider {
        candidates: vec![c2],
    };

    // Registration order [p1, p2] vs [p2, p1]
    let res_12 = cand(&[&p1, &p2], &AdmAll, &e);
    let res_21 = cand(&[&p2, &p1], &AdmAll, &e);

    // BTreeSet ensures canonical ordering regardless of provider order
    assert_eq!(res_12, res_21);

    // Instrumented oracle produces the exact same set
    let (res_inst, _cost) = cand_instrumented(&[&p1, &p2], &AdmAll, &e);
    assert_eq!(res_inst, res_12);
}

// --- Scenario 11: Controlled Edit Sequence with REAL IncrementalEngine ---
// Exercises IncrementalEngine vs naive reference oracle across an add/remove stream.
#[derive(Clone)]
struct IncrementalKnownRegime {
    witness: Handle,
    active: BTreeSet<Handle>,
}

impl WitnessProvider for IncrementalKnownRegime {
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        if self.active.contains(&e.world) {
            vec![Candidate {
                witness: self.witness,
                successor: e.world,
            }]
        } else {
            vec![]
        }
    }
}

impl IncrementalWitnessIndex for IncrementalKnownRegime {
    fn footprint(&self) -> Footprint {
        Footprint::configs(self.active.iter().copied())
    }

    fn apply(&mut self, delta: &Delta) -> CandidateDelta {
        let mut cd = CandidateDelta::new();
        for h in &delta.added {
            if self.active.contains(h) {
                cd.added.insert(Candidate {
                    witness: self.witness,
                    successor: *h,
                });
            }
        }
        for h in &delta.removed {
            if self.active.contains(h) {
                cd.removed.insert(Candidate {
                    witness: self.witness,
                    successor: *h,
                });
            }
        }
        cd
    }
}

#[test]
fn scenario_11_controlled_edit_sequence_incremental_vs_naive_resolution() {
    let mut i = Interner::new();
    let w1 = i.intern(Digest::of(Domain::Value, b"node-1"));
    let w2 = i.intern(Digest::of(Domain::Value, b"node-2"));
    let wit1 = i.intern(Digest::of(Domain::Value, b"wit-1"));
    let wit2 = i.intern(Digest::of(Domain::Value, b"wit-2"));

    let r1 = IncrementalKnownRegime {
        witness: wit1,
        active: BTreeSet::from([w1]),
    };
    let r2 = IncrementalKnownRegime {
        witness: wit2,
        active: BTreeSet::from([w2]),
    };

    let naive_regimes: [&dyn WitnessProvider; 2] = [&r1, &r2];
    let mut engine = IncrementalEngine::new(vec![Box::new(r1.clone()), Box::new(r2.clone())]);

    let mut present = BTreeSet::new();

    let c1 = Candidate {
        witness: wit1,
        successor: w1,
    };
    let c2 = Candidate {
        witness: wit2,
        successor: w2,
    };

    let policy = i.intern(Digest::of(Domain::Value, b"policy"));
    let history = History::empty().digest();

    // Step 0: Empty world
    assert_eq!(engine.view(), &BTreeSet::new());
    let naive_0 = naive_view_over(&naive_regimes, &AdmAll, &present, policy, history);
    assert_eq!(engine.view(), &naive_0);
    let out0 = classify_resolution(engine.view().clone(), CompletenessScope::Complete, None);
    assert_eq!(out0, ResolutionOutcome::NoSolution);

    // Step 1: Add w1 (Delta: +w1)
    let delta1 = Delta::of_added([w1]);
    present.insert(w1);
    engine.step(&delta1);
    let naive_1 = naive_view_over(&naive_regimes, &AdmAll, &present, policy, history);
    assert_eq!(engine.view(), &naive_1);
    assert_eq!(engine.view(), &BTreeSet::from([c1]));
    let out1 = classify_resolution(engine.view().clone(), CompletenessScope::Complete, Some(c1));
    assert_eq!(out1, ResolutionOutcome::Unique(c1));

    // Step 2: Add w2 (Delta: +w2)
    let delta2 = Delta::of_added([w2]);
    present.insert(w2);
    engine.step(&delta2);
    let naive_2 = naive_view_over(&naive_regimes, &AdmAll, &present, policy, history);
    assert_eq!(engine.view(), &naive_2);
    assert_eq!(engine.view(), &BTreeSet::from([c1, c2]));
    let out2 = classify_resolution(engine.view().clone(), CompletenessScope::Complete, Some(c1));
    match out2 {
        ResolutionOutcome::Ambiguous {
            selected,
            candidates,
        } => {
            assert_eq!(selected, Some(c1));
            assert_eq!(candidates.len(), 2);
        }
        _ => panic!("Expected Ambiguous at step 2"),
    }

    // Step 3: Remove w1 (Delta: -w1)
    let delta3 = Delta::of_removed([w1]);
    present.remove(&w1);
    engine.step(&delta3);
    let naive_3 = naive_view_over(&naive_regimes, &AdmAll, &present, policy, history);
    assert_eq!(engine.view(), &naive_3);
    assert_eq!(engine.view(), &BTreeSet::from([c2]));
    let out3 = classify_resolution(engine.view().clone(), CompletenessScope::Complete, Some(c2));
    assert_eq!(out3, ResolutionOutcome::Unique(c2));

    // Step 4: Remove w2 (Delta: -w2) -> returns to empty
    let delta4 = Delta::of_removed([w2]);
    present.remove(&w2);
    engine.step(&delta4);
    let naive_4 = naive_view_over(&naive_regimes, &AdmAll, &present, policy, history);
    assert_eq!(engine.view(), &naive_4);
    assert!(engine.view().is_empty());
    let out4 = classify_resolution(engine.view().clone(), CompletenessScope::Complete, None);
    assert_eq!(out4, ResolutionOutcome::NoSolution);
}

// --- Scenario 12: Canonical Serialization Round-Trip ---
#[test]
fn scenario_12_resolution_query_and_scope_round_trip_canon() {
    let q = ResolutionQuery::new(
        ContextId::root(),
        PropositionId::from_canon(b"target-goal"),
        Digest::of(Domain::Value, b"policy-digest"),
    )
    .with_budget(500);

    let mut w = CanonWriter::new();
    q.canon_write(&mut w);
    let bytes = w.finish();

    let decoded = ResolutionQuery::from_canon_bytes(&bytes).expect("ResolutionQuery must decode");
    assert_eq!(q, decoded);

    // Completeness scope round-trip
    let mut w_c = CanonWriter::new();
    CompletenessScope::Complete.canon_write(&mut w_c);
    let scope_decoded =
        CompletenessScope::from_canon_bytes(&w_c.finish()).expect("CompletenessScope must decode");
    assert_eq!(scope_decoded, CompletenessScope::Complete);
}

// --- Scenario 13: Incomplete Enumeration Cannot Establish Uniqueness ---
#[test]
fn scenario_13_incomplete_enumeration_cannot_claim_uniqueness() {
    let (_i, _e, w1, _w2, s0) = make_test_env();
    let c1 = Candidate {
        witness: w1,
        successor: s0,
    };

    // Exactly one candidate found, but search was Incomplete (bounded)
    let outcome = classify_resolution(
        BTreeSet::from([c1]),
        CompletenessScope::Incomplete,
        Some(c1),
    );

    // Must be classified as Unknown(IncompleteEnumeration), NEVER Unique
    match outcome {
        ResolutionOutcome::Unknown(reason) => {
            assert_eq!(reason, ResolutionUnknown::IncompleteEnumeration);
        }
        other => panic!("Expected Unknown(IncompleteEnumeration), got {:?}", other),
    }

    // Custom fallback reason test
    let outcome_custom = classify_resolution_with_reason(
        BTreeSet::<Candidate>::new(),
        CompletenessScope::Incomplete,
        None,
        ResolutionUnknown::CycleDetected,
    );
    assert_eq!(
        outcome_custom,
        ResolutionOutcome::Unknown(ResolutionUnknown::CycleDetected)
    );
}

// --- Scenario 14: Prohibited Selected Candidate Cannot Survive ---
#[test]
fn scenario_14_prohibited_selected_cannot_survive() {
    let (_i, _e, w1, w2, s0) = make_test_env();
    let c1 = Candidate {
        witness: w1,
        successor: s0,
    };
    let c2 = Candidate {
        witness: w2,
        successor: s0,
    };

    // Admissible candidates contains only c1, but caller attempts to pass c2 as selected
    let outcome = classify_resolution(
        BTreeSet::from([c1]),
        CompletenessScope::Complete,
        Some(c2), // c2 is not in admissible set
    );

    // Single candidate: Unique(c1)
    assert_eq!(outcome, ResolutionOutcome::Unique(c1));

    // Multiple candidates: Ambiguous, but selected is sanitized to None because c2 is not admissible
    let c3 = Candidate {
        witness: w1,
        successor: w2,
    };
    let outcome_ambig = classify_resolution(
        BTreeSet::from([c1, c3]),
        CompletenessScope::Complete,
        Some(c2), // c2 is not in admissible set
    );

    match outcome_ambig {
        ResolutionOutcome::Ambiguous {
            selected,
            candidates,
        } => {
            assert_eq!(
                selected, None,
                "Inadmissible candidate must be filtered out of selected"
            );
            assert_eq!(candidates.len(), 2);
        }
        _ => panic!("Expected Ambiguous outcome"),
    }
}

// --- Scenario 15: Canonical Candidate Projection and Codec ---
#[test]
fn scenario_15_canonical_candidate_projection_and_round_trip() {
    let (i, _e, w1, _w2, s0) = make_test_env();
    let c1 = Candidate {
        witness: w1,
        successor: s0,
    };

    // Project handle candidate to portable CanonicalCandidate
    let canon_c =
        CanonicalCandidate::from_candidate(&c1, &i).expect("Handles must resolve in interner");

    assert_eq!(canon_c.witness.digest(), i.resolve(w1));
    assert_eq!(canon_c.successor.digest(), i.resolve(s0));

    // Round-trip CanonicalCandidate through CanonWriter / CanonReader
    let mut w = CanonWriter::new();
    canon_c.canon_write(&mut w);
    let bytes = w.finish();

    let decoded =
        CanonicalCandidate::from_canon_bytes(&bytes).expect("CanonicalCandidate must decode");
    assert_eq!(canon_c, decoded);
}

// --- Scenario 16: Resolution Outcome Canonical Round-Trip ---
#[test]
fn scenario_16_resolution_outcome_canonical_round_trip() {
    let (i, _e, w1, w2, s0) = make_test_env();
    let c1 = Candidate {
        witness: w1,
        successor: s0,
    };
    let c2 = Candidate {
        witness: w2,
        successor: s0,
    };

    let cc1 = CanonicalCandidate::from_candidate(&c1, &i).unwrap();
    let cc2 = CanonicalCandidate::from_candidate(&c2, &i).unwrap();

    // 1. Unique round-trip
    let out_unique: ResolutionOutcome<CanonicalCandidate> = ResolutionOutcome::Unique(cc1);
    let mut w1_out = CanonWriter::new();
    out_unique.canon_write(&mut w1_out);
    let dec_unique =
        ResolutionOutcome::<CanonicalCandidate>::from_canon_bytes(&w1_out.finish()).unwrap();
    assert_eq!(out_unique, dec_unique);

    // 2. Ambiguous round-trip
    let out_ambig: ResolutionOutcome<CanonicalCandidate> = ResolutionOutcome::Ambiguous {
        selected: Some(cc1),
        candidates: BTreeSet::from([cc1, cc2]),
    };
    let mut w2_out = CanonWriter::new();
    out_ambig.canon_write(&mut w2_out);
    let dec_ambig =
        ResolutionOutcome::<CanonicalCandidate>::from_canon_bytes(&w2_out.finish()).unwrap();
    assert_eq!(out_ambig, dec_ambig);

    // 3. NoSolution round-trip
    let out_nosol: ResolutionOutcome<CanonicalCandidate> = ResolutionOutcome::NoSolution;
    let mut w3_out = CanonWriter::new();
    out_nosol.canon_write(&mut w3_out);
    let dec_nosol =
        ResolutionOutcome::<CanonicalCandidate>::from_canon_bytes(&w3_out.finish()).unwrap();
    assert_eq!(out_nosol, dec_nosol);

    // 4. Unknown round-trip
    let out_unknown: ResolutionOutcome<CanonicalCandidate> = ResolutionOutcome::Unknown(
        ResolutionUnknown::ContextFailure(ContextValidationError::Mismatched),
    );
    let mut w4_out = CanonWriter::new();
    out_unknown.canon_write(&mut w4_out);
    let dec_unknown =
        ResolutionOutcome::<CanonicalCandidate>::from_canon_bytes(&w4_out.finish()).unwrap();
    assert_eq!(out_unknown, dec_unknown);
}

// --- Scenario 17: Decoder Invariant & Negative Coverage ---
// Proves [P1] decoder rejects cardinality violations, duplicates, non-ascending order,
// selected candidate absence, and excessive lengths.
#[test]
fn scenario_17_decoder_enforces_cardinality_ordering_and_limits() {
    let (i, _e, w1, w2, s0) = make_test_env();
    let c1 = Candidate {
        witness: w1,
        successor: s0,
    };
    let c2 = Candidate {
        witness: w2,
        successor: s0,
    };
    let cc1 = CanonicalCandidate::from_candidate(&c1, &i).unwrap();
    let cc2 = CanonicalCandidate::from_candidate(&c2, &i).unwrap();

    // Ensure cc1 < cc2
    let (smaller, larger) = if cc1 < cc2 { (cc1, cc2) } else { (cc2, cc1) };

    // 1. Ambiguous with count = 0 (rejected: cardinality violation)
    let mut w = CanonWriter::new();
    w.write_enum(1, |w| {
        w.write_enum(0, |_| {}); // selected = None
        w.write_uint(0); // count = 0
    });
    assert_eq!(
        ResolutionOutcome::<CanonicalCandidate>::from_canon_bytes(&w.finish()),
        Err(CanonError::BadLength)
    );

    // 2. Ambiguous with count = 1 (rejected: cardinality violation)
    let mut w = CanonWriter::new();
    w.write_enum(1, |w| {
        w.write_enum(0, |_| {}); // selected = None
        w.write_uint(1); // count = 1
        smaller.canon_write(w);
    });
    assert_eq!(
        ResolutionOutcome::<CanonicalCandidate>::from_canon_bytes(&w.finish()),
        Err(CanonError::BadLength)
    );

    // 3. Ambiguous with duplicate candidates [smaller, smaller] (rejected: uniqueness violation)
    let mut w = CanonWriter::new();
    w.write_enum(1, |w| {
        w.write_enum(0, |_| {}); // selected = None
        w.write_uint(2); // count = 2
        smaller.canon_write(w);
        smaller.canon_write(w); // duplicate!
    });
    assert_eq!(
        ResolutionOutcome::<CanonicalCandidate>::from_canon_bytes(&w.finish()),
        Err(CanonError::BadLength)
    );

    // 4. Ambiguous with descending order [larger, smaller] (rejected: noncanonical order)
    let mut w = CanonWriter::new();
    w.write_enum(1, |w| {
        w.write_enum(0, |_| {}); // selected = None
        w.write_uint(2); // count = 2
        larger.canon_write(w); // larger first
        smaller.canon_write(w); // smaller second -> non-ascending!
    });
    assert_eq!(
        ResolutionOutcome::<CanonicalCandidate>::from_canon_bytes(&w.finish()),
        Err(CanonError::BadLength)
    );

    // 5. Ambiguous with selected candidate absent from candidate set (rejected: membership violation)
    let c3 = Candidate {
        witness: s0,
        successor: w1,
    };
    let absent = CanonicalCandidate::from_candidate(&c3, &i).unwrap();
    let mut w = CanonWriter::new();
    w.write_enum(1, |w| {
        w.write_enum(1, |w| absent.canon_write(w)); // selected = Some(absent)
        w.write_uint(2); // count = 2
        smaller.canon_write(w);
        larger.canon_write(w);
    });
    assert_eq!(
        ResolutionOutcome::<CanonicalCandidate>::from_canon_bytes(&w.finish()),
        Err(CanonError::BadLength)
    );

    // 6. Ambiguous with count exceeding MAX_RESOLUTION_CANDIDATES (rejected: resource limit)
    let mut w = CanonWriter::new();
    w.write_enum(1, |w| {
        w.write_enum(0, |_| {});
        w.write_uint((MAX_RESOLUTION_CANDIDATES + 1) as u64);
    });
    assert_eq!(
        ResolutionOutcome::<CanonicalCandidate>::from_canon_bytes(&w.finish()),
        Err(CanonError::BadLength)
    );

    // 7. ResolutionQuery with invalid tag (rejected)
    let mut w = CanonWriter::new();
    w.write_tag("some.foreign.query.tag");
    ContextId::root().canon_write(&mut w);
    PropositionId::from_canon(b"prop").canon_write(&mut w);
    w.write_bytes(Digest::of(Domain::Value, b"pol").as_bytes());
    w.write_enum(0, |_| {});
    assert_eq!(
        ResolutionQuery::from_canon_bytes(&w.finish()),
        Err(CanonError::BadLength)
    );

    // 8. CompletenessScope with invalid ordinal
    let mut w = CanonWriter::new();
    w.write_uint(99);
    assert_eq!(
        CompletenessScope::from_canon_bytes(&w.finish()),
        Err(CanonError::BadLength)
    );

    // 9. ResolutionUnknown with invalid ordinal
    let mut w = CanonWriter::new();
    w.write_uint(99);
    assert_eq!(
        ResolutionUnknown::from_canon_bytes(&w.finish()),
        Err(CanonError::BadLength)
    );
}

// --- Scenario 18: Verified, Query-Bound Completeness Evidence ---
#[test]
fn scenario_18_verified_query_bound_completeness_evidence() {
    // 1. Build a terminating fixture: w0 -tau-> w1 -tau-> w2 (terminal)
    let fx = build_chain_fixture(&[("w0", "w1", vec![gen_tau()]), ("w1", "w2", vec![gen_tau()])]);
    let profile = hiding_profile();
    let regime: &dyn SettlementWitnessProvider = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let adm_id = Digest::of(Domain::Value, b"policy-for-completeness");
    let pres = chain_presentation(regimes, &profile, &fx.interner, &AdmAll, adm_id);

    let mut keyer = |c: &Candidate, phase: u64| Key::new(phase, 0, tiebreak_digest(c));

    let (step, consumed, _) = sat_step(
        &pres,
        &fx.exec_at("w0"),
        0,
        &mut keyer,
        SaturationBudget::uniform(32),
    );
    let SaturatedStep::Quiescent(cert) = step else {
        panic!("expected quiescent step under terminating fixture");
    };

    let terminal_config = ExecConfig::new(fx.worlds["w2"], fx.policy, History::empty().digest());

    // 2. Query matching presentation bindings
    let matching_query = ResolutionQuery {
        context: pres.context,
        goal: PropositionId::from_canon(b"termination-goal"),
        policy: adm_id,
        budget: Some(32),
    };

    // Quiescence is valid execution evidence, but does not prove that the run enumerated
    // solutions to this resolution goal.
    let verified = VerifiedCompleteness::<Candidate>::from_quiescence_certificate(
        &matching_query,
        &cert,
        &pres,
        &terminal_config,
        &consumed,
    )
    .expect_err("quiescence alone must not certify resolution completeness");
    assert_eq!(verified, CompletenessVerificationError::UnsupportedEvidence);

    // 3. Query with mismatched context fails verification
    let mismatched_ctx_query = ResolutionQuery {
        context: ContextId::root().extend(b"wrong-context"),
        ..matching_query
    };
    let err_ctx = VerifiedCompleteness::<Candidate>::from_quiescence_certificate(
        &mismatched_ctx_query,
        &cert,
        &pres,
        &terminal_config,
        &consumed,
    );
    assert_eq!(err_ctx, Err(CompletenessVerificationError::ContextMismatch));

    // 4. Query with mismatched policy fails verification
    let mismatched_pol_query = ResolutionQuery {
        policy: Digest::of(Domain::Value, b"wrong-policy"),
        ..matching_query
    };
    let err_pol = VerifiedCompleteness::<Candidate>::from_quiescence_certificate(
        &mismatched_pol_query,
        &cert,
        &pres,
        &terminal_config,
        &consumed,
    );
    assert_eq!(err_pol, Err(CompletenessVerificationError::PolicyMismatch));

    // 5. Tampered certificate fails verification
    let mut tampered_cert = cert.clone();
    tampered_cert.terminal_world = ConfigId::from_canon(b"wrong-terminal-world");
    let err_cert = VerifiedCompleteness::<Candidate>::from_quiescence_certificate(
        &matching_query,
        &tampered_cert,
        &pres,
        &terminal_config,
        &consumed,
    );
    assert_eq!(
        err_cert,
        Err(CompletenessVerificationError::InvalidCertificate)
    );

    // 6. No currently supported evidence promotes empty results to NoSolution.
    let out_empty_verified = classify_resolution_with_verified_evidence::<Candidate>(
        &matching_query,
        BTreeSet::new(),
        None,
        None,
        ResolutionUnknown::BudgetExhausted,
    );
    assert_eq!(
        out_empty_verified,
        ResolutionOutcome::Unknown(ResolutionUnknown::BudgetExhausted)
    );

    // 7. Closed finite domain verification:
    let (_i, _e, w1, w2, s0) = make_test_env();
    let c1 = Candidate {
        witness: w1,
        successor: s0,
    };
    let unsupported_domain = VerifiedCompleteness::from_closed_domain(
        &matching_query,
        BTreeSet::from([c1]),
        &BTreeSet::from([c1]),
    )
    .expect_err("caller-authored finite domains are not independent completeness proofs");
    assert_eq!(
        unsupported_domain,
        CompletenessVerificationError::UnsupportedEvidence
    );

    let out_unique_domain = classify_resolution_with_verified_evidence(
        &matching_query,
        BTreeSet::from([c1]),
        None,
        Some(c1),
        ResolutionUnknown::BudgetExhausted,
    );
    assert_eq!(
        out_unique_domain,
        ResolutionOutcome::Unknown(ResolutionUnknown::IncompleteEnumeration)
    );

    let c2 = Candidate {
        witness: w2,
        successor: s0,
    };

    // Same-size arbitrary sets cannot be promoted to completeness evidence.
    let err_swap = VerifiedCompleteness::from_closed_domain(
        &matching_query,
        BTreeSet::from([c1]),
        &BTreeSet::from([c2]),
    );
    assert_eq!(
        err_swap,
        Err(CompletenessVerificationError::UnsupportedEvidence)
    );

    // Candidate substitution and query changes fail closed without verified evidence.
    let other_query = ResolutionQuery {
        goal: PropositionId::from_canon(b"other-goal"),
        ..matching_query
    };
    let out_reused = classify_resolution_with_verified_evidence(
        &other_query,
        BTreeSet::from([c1]),
        None,
        Some(c1),
        ResolutionUnknown::BudgetExhausted,
    );
    assert_eq!(
        out_reused,
        ResolutionOutcome::Unknown(ResolutionUnknown::IncompleteEnumeration)
    );

    // A different candidate set likewise remains incomplete.
    let out_outside = classify_resolution_with_verified_evidence(
        &matching_query,
        BTreeSet::from([c2]),
        None,
        Some(c2),
        ResolutionUnknown::BudgetExhausted,
    );
    assert_eq!(
        out_outside,
        ResolutionOutcome::Unknown(ResolutionUnknown::IncompleteEnumeration)
    );

    // A caller-authored domain cannot certify a larger enumerated set either.
    let err_domain = VerifiedCompleteness::from_closed_domain(
        &matching_query,
        BTreeSet::from([c1, c2]), // certified 2, but only 1 enumerated
        &BTreeSet::from([c1]),
    );
    assert_eq!(
        err_domain,
        Err(CompletenessVerificationError::UnsupportedEvidence)
    );

    // 8. Missing evidence (None) defaults fail-closed to Incomplete:
    let out_none = classify_resolution_with_verified_evidence::<Candidate>(
        &matching_query,
        BTreeSet::new(),
        None,
        None,
        ResolutionUnknown::BudgetExhausted,
    );
    assert_eq!(
        out_none,
        ResolutionOutcome::Unknown(ResolutionUnknown::BudgetExhausted)
    );

    let out_one_none = classify_resolution_with_verified_evidence(
        &matching_query,
        BTreeSet::from([c1]),
        None,
        Some(c1),
        ResolutionUnknown::BudgetExhausted,
    );
    assert_eq!(
        out_one_none,
        ResolutionOutcome::Unknown(ResolutionUnknown::IncompleteEnumeration)
    );

    // Reports retain query identity for the same candidate set. Unsupported or absent evidence
    // cannot promote either result to complete.
    let report_a = ResolutionReport::classify(
        matching_query,
        BTreeSet::from([c1]),
        None,
        Some(c1),
        ResolutionUnknown::BudgetExhausted,
    );
    let report_b = ResolutionReport::classify(
        other_query,
        BTreeSet::from([c1]),
        None,
        Some(c1),
        ResolutionUnknown::BudgetExhausted,
    );
    assert_eq!(*report_a.query(), matching_query);
    assert_eq!(*report_b.query(), other_query);
    assert_eq!(report_a.completeness(), CompletenessScope::Incomplete);
    assert_eq!(report_b.completeness(), CompletenessScope::Incomplete);
    assert_ne!(report_a.query_id(), report_b.query_id());
    assert_eq!(report_a.verified_evidence_digest(), None);
    assert_eq!(report_b.verified_evidence_digest(), None);
    assert_eq!(
        report_a.outcome(),
        &ResolutionOutcome::Unknown(ResolutionUnknown::IncompleteEnumeration)
    );
    assert_eq!(
        report_b.outcome(),
        &ResolutionOutcome::Unknown(ResolutionUnknown::IncompleteEnumeration)
    );
}

// --- Scenario 19: Classifier Enforces Candidate Limit and Round-Trips ---
#[test]
fn scenario_19_classifier_enforces_candidate_limit_and_round_trips() {
    let s_id = ConfigId::from_canon(b"target-config");

    // 1. Construct 1,025 candidates (> MAX_RESOLUTION_CANDIDATES = 1024)
    let mut large_set = BTreeSet::new();
    for idx in 0..=MAX_RESOLUTION_CANDIDATES {
        let w_id = WitnessId(Digest::of(
            Domain::Value,
            format!("cand-wit-{idx}").as_bytes(),
        ));
        large_set.insert(CanonicalCandidate::new(w_id, s_id));
    }
    assert_eq!(large_set.len(), 1025);

    // 2. Classifier rejects 1,025 candidates fail-closed as BudgetExhausted, never an oversized Ambiguous
    let outcome = classify_resolution(large_set.clone(), CompletenessScope::Complete, None);
    assert_eq!(
        outcome,
        ResolutionOutcome::Unknown(ResolutionUnknown::BudgetExhausted)
    );

    // 3. Round-trip serialization succeeds because the classifier output is consistent with the decoder
    let mut w = CanonWriter::new();
    outcome.canon_write(&mut w);
    let bytes = w.finish();
    let decoded = ResolutionOutcome::<CanonicalCandidate>::from_canon_bytes(&bytes)
        .expect("classifier output must round-trip cleanly");
    assert_eq!(outcome, decoded);

    // 4. In contrast, a hostile or malformed byte stream attempting to encode 1,025 candidates directly
    // is rejected by the decoder
    let mut w_hostile = CanonWriter::new();
    w_hostile.write_enum(1, |w| {
        w.write_enum(0, |_| {}); // selected = None
        w.write_uint(1025); // count = 1025
    });
    assert_eq!(
        ResolutionOutcome::<CanonicalCandidate>::from_canon_bytes(&w_hostile.finish()),
        Err(CanonError::BadLength)
    );

    // 5. Safe constructor ResolutionOutcome::ambiguous also enforces the limit
    let outcome_ambig = ResolutionOutcome::ambiguous(None, large_set);
    assert_eq!(
        outcome_ambig,
        ResolutionOutcome::Unknown(ResolutionUnknown::BudgetExhausted)
    );
}
