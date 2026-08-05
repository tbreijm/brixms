//! Checker fixture suite for the audit-factorization checker (Lane 2;
//! ADR-0002 §4.1, §5 point 1; `Build_Plan_v3_SOC.md` Step 4 gate).
//!
//! Builds one real end-to-end fixture: a [`SettlementRegime`] whose recorded
//! [`Decomposition`] is a genuine `𝒢`-chain with **two** generators (so the
//! relational composition being verified is non-trivial), driven through
//! Lane 1's `commit::run` to produce a real [`Journal`]/`CommittedStep` —
//! then audited via [`soc_core::audit::audit_step`]. The four mandatory
//! fixtures below are the task gate:
//!
//! (a) a good decomposition upgrades to `Audited` with the correct authority;
//! (b) a corrupted intermediate configuration yields `Unknown(_)`, never a
//!     pass;
//! (c) a generator outside `𝒢` yields `Unknown(_)`, never a pass;
//! (d) the `Derived` judgement is never mutated — the upgrade is a new
//!     judgement plus a `Dependency` edge, and the `Audited`/`Derived`
//!     evidence differ.

use std::collections::BTreeSet;

use brix_canon::{Digest, Domain};
use brix_semantic::{
    Authority, ConfigId, ContextId, Decomposition, Evidence, GeneratorId, GeneratorRegistry,
    Judgement, Outcome, Realizes,
};
use soc_core::adm::AdmAll;
use soc_core::audit::{audit_step, AuditResult, GeneratorSemantics};
use soc_core::calendar::Key;
use soc_core::commit::{run, CommitError, SettlementRegime};
use soc_core::exec::ExecConfig;
use soc_core::history::History;
use soc_core::intern::{Handle, Interner};
use soc_core::journal::CommittedStep;
use soc_core::regime::{Candidate, Regime};

/// A single-candidate fixture regime whose recorded `Decomposition` is a
/// genuine two-generator chain `x0 --g1--> x1 --g2--> x2` — non-trivial
/// composition, exercising the stepwise `ρ_k = ρ_g2 ∘ ρ_g1` check.
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

fn gen1() -> GeneratorId {
    GeneratorId::named("audit-fixture.g1@1")
}
fn gen2() -> GeneratorId {
    GeneratorId::named("audit-fixture.g2@1")
}
fn cfg_x0() -> ConfigId {
    ConfigId::from_canon(b"audit-fixture-x0")
}
fn cfg_x1() -> ConfigId {
    ConfigId::from_canon(b"audit-fixture-x1")
}
fn cfg_x2() -> ConfigId {
    ConfigId::from_canon(b"audit-fixture-x2")
}

fn fixture_decomposition() -> Decomposition {
    Decomposition::recorded(vec![gen1(), gen2()], vec![cfg_x0(), cfg_x1(), cfg_x2()]).unwrap()
}

impl SettlementRegime for FixtureRegime {
    fn try_decompose(&self, _e: &ExecConfig, _c: &Candidate) -> Result<Decomposition, CommitError> {
        Ok(fixture_decomposition())
    }
}

fn tiebreak_of(c: &Candidate) -> Digest {
    let mut w = brix_canon::CanonWriter::new();
    w.write_uint(c.witness.raw() as u64);
    w.write_uint(c.successor.raw() as u64);
    w.digest(Domain::Value)
}

/// Sets up an `Interner` whose world/successor handles resolve to exactly
/// `cfg_x0()`/`cfg_x2()`'s underlying digests, so the committed step's
/// `src`/`dst` land on the fixture decomposition's endpoints — the same
/// wrap-verbatim discipline `commit::commit_tick` documents (the interned
/// digest already *is* the canonical `ConfigId`/`WitnessId` identity, no
/// re-hash).
fn setup() -> (Interner, FixtureRegime, ExecConfig) {
    let mut i = Interner::new();
    let world = i.intern(cfg_x0().digest());
    let policy = i.intern(Digest::of(Domain::Value, b"audit-fixture-p0"));
    let regime = i.intern(Digest::of(Domain::Value, b"audit-fixture-r"));
    let witness = i.intern(Digest::of(Domain::Value, b"audit-fixture-witness"));
    let successor = i.intern(cfg_x2().digest());
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

/// A deterministic, map-backed [`GeneratorSemantics`]: a `BTreeSet` of
/// realized `(generator, src, dst)` triples — never a `HashSet` (workspace
/// determinism policy).
struct FixtureSemantics(BTreeSet<(GeneratorId, ConfigId, ConfigId)>);

impl FixtureSemantics {
    fn correct() -> Self {
        let mut set = BTreeSet::new();
        set.insert((gen1(), cfg_x0(), cfg_x1()));
        set.insert((gen2(), cfg_x1(), cfg_x2()));
        FixtureSemantics(set)
    }
}

impl GeneratorSemantics for FixtureSemantics {
    fn realizes(&self, g: &GeneratorId, src: &ConfigId, dst: &ConfigId) -> bool {
        self.0.contains(&(*g, *src, *dst))
    }
}

fn registry_with(gens: &[GeneratorId]) -> GeneratorRegistry {
    let mut r = GeneratorRegistry::new();
    for g in gens {
        r.insert(*g);
    }
    r
}

/// Drive the real `commit::run` loop for exactly one tick to produce a
/// genuine `CommittedStep` (not a hand-built one) — the same entry point any
/// real regime goes through.
fn committed_fixture_step() -> (CommittedStep, ContextId) {
    let (i, regime, e) = setup();
    let regimes: Vec<&dyn SettlementRegime> = vec![&regime];
    let context = ContextId::root();
    let keyer = |c: &Candidate, phase: u64| Key::new(phase, 0, tiebreak_of(c));

    let (journal, _costs) = run(&regimes, &AdmAll, &i, e, context, keyer, 1);
    assert_eq!(journal.len(), 1, "exactly one committed tick expected");
    (journal.steps()[0].clone(), context)
}

/// Independently rebuild the `Derived` judgement id for `step` — used both
/// to sanity-check the fixture and, in test (d), to prove the audit never
/// mutates it.
fn rebuild_derived_id(step: &CommittedStep, context: ContextId) -> brix_semantic::JudgementId {
    let proposition = Realizes::new(step.witness, step.src, step.dst).proposition_id();
    let derived_evidence = Evidence::SettlementReplay {
        body: step.decomposition.id().digest(),
    }
    .id();
    Judgement::new(context, proposition, Outcome::Derived, derived_evidence).id()
}

// --- (a) Good decomposition upgrades to `Audited` with correct authority ---

#[test]
fn good_decomposition_upgrades_to_audited_with_correct_authority() {
    let (step, context) = committed_fixture_step();
    let registry = registry_with(&[gen1(), gen2()]);
    let semantics = FixtureSemantics::correct();

    match audit_step(&step, context, &registry, &semantics) {
        AuditResult::Audited(audited_step) => {
            assert_eq!(audited_step.audited.outcome, Outcome::Audited);
            assert_eq!(Outcome::Audited.authority(), Authority::AuditChecker);
            assert!(
                audited_step.verified.is_replay_verified(),
                "the upgraded decomposition must be in ReplayVerified form"
            );
        }
        AuditResult::Unknown(reason) => {
            panic!("expected Audited for a correct decomposition, got Unknown({reason})")
        }
    }
}

// --- (b) Corrupted intermediate configuration ⇒ Unknown(reason) ---

#[test]
fn corrupted_intermediate_configuration_yields_unknown_never_a_pass() {
    let (step, context) = committed_fixture_step();
    let registry = registry_with(&[gen1(), gen2()]);
    let semantics = FixtureSemantics::correct();

    // Tamper the single intermediate config (x1 -> a config no generator
    // realizes) while keeping the endpoints (x0, x2) and the generator chain
    // intact, and recompute the observation to stay internally consistent
    // with the tampered decomposition — isolating the failure to the
    // relational-composition check (step 3), not the log-integrity check
    // (step 1).
    let tampered_configs = vec![
        cfg_x0(),
        ConfigId::from_canon(b"audit-fixture-wrong-x1"),
        cfg_x2(),
    ];
    let tampered_decomposition =
        Decomposition::recorded(vec![gen1(), gen2()], tampered_configs).unwrap();

    let proposition = Realizes::new(step.witness, step.src, step.dst).proposition_id();
    let tampered_evidence = Evidence::SettlementReplay {
        body: tampered_decomposition.id().digest(),
    }
    .id();
    let tampered_derived_id =
        Judgement::new(context, proposition, Outcome::Derived, tampered_evidence).id();

    let tampered_step = CommittedStep {
        key: step.key,
        observation: soc_core::commit::Observation {
            outcome_class: Outcome::Derived,
            judgement_digest: tampered_derived_id.digest(),
        },
        decomposition: tampered_decomposition,
        src: step.src,
        dst: step.dst,
        witness: step.witness,
    };

    let result = audit_step(&tampered_step, context, &registry, &semantics);
    assert!(
        matches!(result, AuditResult::Unknown(_)),
        "a corrupted intermediate configuration must never yield Audited"
    );
}

// --- (c) A generator outside 𝒢 ⇒ Unknown(reason) ---

#[test]
fn generator_outside_registry_yields_unknown_never_a_pass() {
    let (step, context) = committed_fixture_step();
    // g2 is cited by the decomposition but deliberately absent from 𝒢.
    let registry = registry_with(&[gen1()]);
    let semantics = FixtureSemantics::correct();

    let result = audit_step(&step, context, &registry, &semantics);
    assert!(
        matches!(result, AuditResult::Unknown(_)),
        "a generator outside the registered 𝒢 must never yield Audited"
    );
}

// --- (d) The Derived judgement is never mutated: a NEW judgement + edge ---

#[test]
fn derived_judgement_is_never_mutated_upgrade_is_a_new_judgement_and_edge() {
    let (step, context) = committed_fixture_step();
    let registry = registry_with(&[gen1(), gen2()]);
    let semantics = FixtureSemantics::correct();

    let derived_id_before = rebuild_derived_id(&step, context);

    let audited_step = match audit_step(&step, context, &registry, &semantics) {
        AuditResult::Audited(a) => a,
        AuditResult::Unknown(reason) => panic!("expected Audited, got Unknown({reason})"),
    };

    let derived_id_after = rebuild_derived_id(&step, context);

    // The audit produces a new artifact; it does not alter the Derived one.
    assert_eq!(
        derived_id_before, derived_id_after,
        "the Derived judgement must be byte-identical before and after the audit"
    );
    assert_eq!(audited_step.derived_id, derived_id_before);

    // A NEW judgement, distinct JudgementId, linked by a Dependency edge.
    assert_ne!(
        audited_step.audited_id, audited_step.derived_id,
        "Audited must be a different JudgementId from Derived"
    );
    assert_eq!(audited_step.link.kind, brix_semantic::EdgeKind::Premise);
    assert_eq!(audited_step.link.target, audited_step.derived_id.digest());

    // The Audited evidence differs from the Derived evidence (different
    // DecompositionId: ReplayVerified vs Recorded over identical data).
    let derived_evidence = Evidence::SettlementReplay {
        body: step.decomposition.id().digest(),
    }
    .id();
    let audited_evidence = Evidence::SettlementReplay {
        body: audited_step.verified.id().digest(),
    }
    .id();
    assert_ne!(
        derived_evidence, audited_evidence,
        "Audited evidence must differ from Derived evidence"
    );
    assert_eq!(audited_step.audited.evidence, audited_evidence);
}

// --- Extra: log-integrity mismatch ⇒ Unknown (not one of the four required
// fixtures, but a useful direct check on step 1 of the procedure) ---

#[test]
fn log_integrity_mismatch_yields_unknown() {
    let (step, context) = committed_fixture_step();
    let registry = registry_with(&[gen1(), gen2()]);
    let semantics = FixtureSemantics::correct();

    let mut corrupted = step.clone();
    corrupted.observation.judgement_digest = Digest::of(Domain::Value, b"not-the-real-digest");

    let result = audit_step(&corrupted, context, &registry, &semantics);
    assert!(matches!(result, AuditResult::Unknown(_)));
}

#[test]
fn non_recorded_decomposition_is_rejected() {
    let (step, context) = committed_fixture_step();
    let registry = registry_with(&[gen1(), gen2()]);
    let semantics = FixtureSemantics::correct();

    let already_verified =
        Decomposition::replay_verified(vec![gen1(), gen2()], vec![cfg_x0(), cfg_x1(), cfg_x2()])
            .unwrap();
    let mut corrupted = step.clone();
    corrupted.decomposition = already_verified;

    let result = audit_step(&corrupted, context, &registry, &semantics);
    assert!(matches!(result, AuditResult::Unknown(_)));
}
