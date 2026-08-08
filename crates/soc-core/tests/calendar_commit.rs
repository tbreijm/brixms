//! Calendar + commit integration gates (`Build_Plan_v3_SOC.md` Step 4,
//! S3⋈E4; ADR-0002 §1, §8, §9.2). Exercises the public API only, the way an
//! external consumer (e.g. Lane 2's audit-factorization checker) would:
//!
//! 1. `select_K` totality — a unique least key under a digest tie-break.
//! 2. The B^uk unique-key discipline — divergent duplicate rejected,
//!    idempotent duplicate accepted.
//! 3. The committed coalgebra `γ = select_K ∘ δ` — `Committed::Step` with
//!    `Outcome::Derived` when admissible, `Committed::Quiescent` otherwise.
//! 4. Deterministic replay — running the committed loop twice from the same
//!    inputs yields byte-identical `Journal::step_digests()`/
//!    `chain_digest()`, and `Journal::replay_chain` reproduces them
//!    independently.
//! 5. A `CostRecord` is emitted for every committed tick, never omitted.
//! 6. The committed `Observation.judgement_digest` matches an independently
//!    hand-rebuilt `Derived` `JudgementId`.

use brix_canon::{CanonWriter, Digest, Domain};
use brix_semantic::{
    ConfigId, ContextId, Decomposition, Evidence, GeneratorId, JudgementId, Outcome, Realizes,
};
use soc_core::adm::{AdmAll, AdmNone};
use soc_core::calendar::{Frontier, Key};
use soc_core::commit::{commit_tick, run, CommitError, Committed, SettlementRegime};
use soc_core::exec::ExecConfig;
use soc_core::history::History;
use soc_core::intern::{Handle, Interner};
use soc_core::journal::Journal;
use soc_core::regime::{Candidate, Regime};

/// A single-candidate fixture regime. Its candidate and its recorded
/// `Decomposition` are both constant, which keeps the deterministic-replay
/// and independent-rebuild checks simple without weakening what they prove:
/// the fixture is exercised through the same public `commit_tick`/`run`
/// entry points any real regime would go through.
struct FixtureRegime {
    id: Handle,
    witness: Handle,
    successor: Handle,
}

impl Regime for FixtureRegime {
    fn candidates(&self, _e: &ExecConfig) -> Vec<Candidate> {
        vec![Candidate {
            regime: self.id,
            witness: self.witness,
            successor: self.successor,
        }]
    }
}

fn fixture_decomposition() -> Decomposition {
    Decomposition::recorded(
        vec![GeneratorId::named("calendar-commit-fixture.step@1")],
        vec![
            ConfigId::from_canon(b"cc-fixture-x0"),
            ConfigId::from_canon(b"cc-fixture-x1"),
        ],
    )
    .unwrap()
}

impl SettlementRegime for FixtureRegime {
    fn try_decompose(&self, _e: &ExecConfig, _c: &Candidate) -> Result<Decomposition, CommitError> {
        Ok(fixture_decomposition())
    }
}

/// A canonical digest derived from a candidate's own handles — stable within
/// one run of a fixed `Interner`, and unique per distinct candidate in these
/// single-candidate-per-tick fixtures. This is the "digest of witness+
/// successor handles" tie-break the design doc calls out for fixtures.
fn tiebreak_of(c: &Candidate) -> Digest {
    let mut w = CanonWriter::new();
    w.write_uint(c.witness.raw() as u64);
    w.write_uint(c.successor.raw() as u64);
    w.digest(Domain::Value)
}

fn setup() -> (Interner, FixtureRegime, ExecConfig) {
    let mut i = Interner::new();
    let world = i.intern(Digest::of(Domain::Value, b"cc-w0"));
    let policy = i.intern(Digest::of(Domain::Value, b"cc-p0"));
    let regime = i.intern(Digest::of(Domain::Value, b"cc-r"));
    let witness = i.intern(Digest::of(Domain::Value, b"cc-wit"));
    let successor = i.intern(Digest::of(Domain::Value, b"cc-w1"));
    let e = ExecConfig::new(world, policy, History::empty().digest());
    (
        i,
        FixtureRegime {
            id: regime,
            witness,
            successor,
        },
        e,
    )
}

// --- 1. select_K totality: equal phase+priority, digest tie-break decides ---

#[test]
fn select_k_totality_under_equal_phase_and_priority() {
    let a = Digest::of(Domain::Value, b"candidate-a");
    let b = Digest::of(Domain::Value, b"candidate-b");
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };

    let mut frontier = Frontier::new();
    frontier.insert(Key::new(0, 0, hi), "second").unwrap();
    frontier.insert(Key::new(0, 0, lo), "first").unwrap();

    let (key, value) = frontier
        .select_least()
        .expect("select_K must find a unique least key");
    assert_eq!(
        key.tiebreak, lo,
        "the smaller digest must win the tie-break"
    );
    assert_eq!(value, "first");
}

// --- 2. B^uk unique-key discipline ---

#[test]
fn buk_divergent_duplicate_key_is_rejected_idempotent_duplicate_is_accepted() {
    let mut frontier = Frontier::new();
    let key = Key::new(1, 1, Digest::of(Domain::Value, b"shared-key"));

    assert!(frontier.insert(key, 100u64).unwrap());
    // Idempotent: same key, same value — accepted, no-op.
    assert!(!frontier.insert(key, 100u64).unwrap());
    assert_eq!(frontier.len(), 1);

    // Divergent: same key, different value — rejected.
    let conflict = frontier
        .insert(key, 200u64)
        .expect_err("a duplicate key mapping to a different value must be rejected");
    assert_eq!(conflict.existing, 100);
    assert_eq!(conflict.attempted, 200);
    assert_eq!(
        frontier.len(),
        1,
        "a rejected insert must not mutate the frontier"
    );
}

// --- 3. The committed coalgebra ---

#[test]
fn committed_coalgebra_steps_when_admissible_and_is_quiescent_otherwise() {
    let (i, regime, e) = setup();
    let regimes: Vec<&dyn SettlementRegime> = vec![&regime];

    let (committed, step, cost) = commit_tick(
        &regimes,
        &AdmAll,
        &i,
        &e,
        ContextId::root(),
        0,
        &mut |c, phase| Key::new(phase, 0, tiebreak_of(c)),
    );
    match committed {
        Committed::Step { observation, .. } => {
            assert_eq!(observation.outcome_class, Outcome::Derived);
        }
        Committed::Quiescent => panic!("one admissible candidate must commit, not quiesce"),
    }
    assert!(step.is_some());
    assert!(cost.work_units().is_some());

    let (committed_none, step_none, cost_none) = commit_tick(
        &regimes,
        &AdmNone,
        &i,
        &e,
        ContextId::root(),
        0,
        &mut |c, phase| Key::new(phase, 0, tiebreak_of(c)),
    );
    assert_eq!(committed_none, Committed::Quiescent);
    assert!(step_none.is_none());
    assert!(
        cost_none.work_units().is_some(),
        "cost is never omitted, even on quiescence"
    );
}

// --- 4. Deterministic replay (byte-identical) ---

#[test]
fn deterministic_replay_is_byte_identical_across_two_runs() {
    let (i, regime, e) = setup();
    let regimes: Vec<&dyn SettlementRegime> = vec![&regime];
    let context = ContextId::root();
    let keyer = |c: &Candidate, phase: u64| Key::new(phase, 0, tiebreak_of(c));

    let (journal_a, costs_a) = run(&regimes, &AdmAll, &i, e, context, keyer, 3);
    let (journal_b, costs_b) = run(&regimes, &AdmAll, &i, e, context, keyer, 3);

    assert!(
        !journal_a.is_empty(),
        "fixture must actually commit at least once"
    );
    assert_eq!(journal_a.step_digests(), journal_b.step_digests());
    assert_eq!(journal_a.chain_digest(), journal_b.chain_digest());
    assert_eq!(costs_a, costs_b);

    // Journal::replay_chain independently reproduces the same running
    // digests from a fresh History fold over the logged steps.
    assert_eq!(
        Journal::replay_chain(journal_a.steps()),
        journal_a.step_digests()
    );
}

// --- 5. Per-tick CostRecord, never omitted ---

#[test]
fn a_cost_record_is_emitted_for_every_committed_tick() {
    let (i, regime, e) = setup();
    let regimes: Vec<&dyn SettlementRegime> = vec![&regime];
    let keyer = |c: &Candidate, phase: u64| Key::new(phase, 0, tiebreak_of(c));

    let (journal, costs) = run(&regimes, &AdmAll, &i, e, ContextId::root(), keyer, 4);

    assert_eq!(
        costs.len(),
        journal.len(),
        "exactly one CostRecord per committed tick, none omitted"
    );
    assert!(!costs.is_empty());
    for cost in &costs {
        assert!(cost.work_units().is_some());
    }
}

// --- 6. Observation.judgement_digest matches an independent rebuild ---

#[test]
fn committed_observation_judgement_digest_matches_independent_reconstruction() {
    let (i, regime, e) = setup();
    let regimes: Vec<&dyn SettlementRegime> = vec![&regime];
    let context = ContextId::root();

    let (committed, _step, _cost) =
        commit_tick(&regimes, &AdmAll, &i, &e, context, 0, &mut |c, phase| {
            Key::new(phase, 0, tiebreak_of(c))
        });
    let Committed::Step { observation, .. } = committed else {
        panic!("expected a committed step");
    };

    // Rebuild the Realizes -> Decomposition -> Evidence -> Judgement chain
    // by hand, using only public constructors, independent of commit_tick's
    // internals.
    let src = ConfigId(i.resolve(e.world));
    let dst = ConfigId(i.resolve(regime.successor));
    let decomposition = fixture_decomposition();
    let witness = brix_semantic::compose_chain(&decomposition.generators).unwrap();
    let proposition = Realizes::new(witness, src, dst).proposition_id();
    let evidence = Evidence::SettlementReplay {
        body: decomposition.id().digest(),
    }
    .id();
    let judgement_id = JudgementId::recompute(context, proposition, Outcome::Derived, evidence);

    assert_eq!(observation.outcome_class, Outcome::Derived);
    assert_eq!(
        observation.judgement_digest,
        judgement_id.digest(),
        "the committed Observation must carry exactly the independently-rebuilt Derived JudgementId's digest"
    );
}
