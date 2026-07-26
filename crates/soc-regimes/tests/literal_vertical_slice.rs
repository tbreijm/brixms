//! The first full vertical slice of the SOC loop (`spec/Build_Plan_v3_SOC.md`
//! Step 5(a); ADR-0002 §7): candidates → `Adm` → calendar/commit → a
//! `Derived` `Realizes` judgement → the audit-factorization checker's
//! upgrade to `Audited`, all driven by the simplest possible regime — literal
//! equality.
//!
//! This does not re-test `soc-core`'s own machinery (calendar totality,
//! B^uk uniqueness, replay, …) — those gates live in `soc-core`'s own test
//! suite. What this test proves is specific to this crate: that a *real*,
//! non-fixture [`soc_regimes::LiteralEqualityRegime`] plugged into
//! `soc_core::run` produces a `Derived` step whose recorded decomposition
//! the audit-factorization checker actually verifies and upgrades to
//! `Audited` — the end-to-end proof that this regime's witness/decomposition
//! construction (module docs on `literal.rs`) is wired correctly, not just
//! unit-correct in isolation.

use brix_canon::{CanonWriter, Digest, Domain};
use brix_semantic::{ContextId, GeneratorId, GeneratorRegistry, Outcome};
use soc_core::{
    audit_step, intern_context, run, AdmAll, AuditResult, Candidate, ExecConfig, History, Interner,
    Key,
};
use soc_regimes::{LiteralEqualityRegime, LiteralEqualitySemantics};

/// Mirrors `soc-core`'s own `commit.rs` test helper `tiebreak_of`: a
/// canonical digest derived from the candidate's own handles, stable within
/// one run of a fixed `Interner` — sufficient to make the calendar
/// tie-break unique in this single-candidate-per-tick vertical slice.
fn tiebreak_of(c: &Candidate) -> Digest {
    let mut w = CanonWriter::new();
    w.write_uint(c.witness.raw() as u64);
    w.write_uint(c.successor.raw() as u64);
    w.digest(Domain::Value)
}

#[test]
fn literal_equality_derives_then_audits_the_reflexive_witness() {
    // --- Build the world: an ExecConfig rooted at ContextId::root(). ---
    let mut interner = Interner::new();
    let world = intern_context(&mut interner, ContextId::root());
    let policy = interner.intern(Digest::of(Domain::Value, b"policy"));
    let e0 = ExecConfig::new(world, policy, History::empty().digest());
    let context = ContextId::root();

    // --- The regime: literal equality, registered for this one world. ---
    let mut regime = LiteralEqualityRegime::new(&mut interner);
    regime.register(&mut interner, world);
    let regimes: Vec<&dyn soc_core::SettlementRegime> = vec![&regime];

    // --- Run the committed loop: one tick, since the reflexive witness is
    // a self-loop (world -> world) and would otherwise commit forever. ---
    let (journal, costs) = run(
        &regimes,
        &AdmAll,
        &interner,
        e0,
        context,
        |c, phase| Key::new(phase, 0, tiebreak_of(c)),
        1,
    );

    assert_eq!(journal.len(), 1, "exactly one committed Derived step");
    assert_eq!(costs.len(), 1);

    let step = &journal.steps()[0];
    assert_eq!(
        step.observation.outcome_class,
        Outcome::Derived,
        "the committed loop's own outcome class is always Derived"
    );

    // --- Audit: replay the recorded decomposition and verify it upgrades
    // to Audited. ---
    let mut registry = GeneratorRegistry::new();
    registry.insert(GeneratorId::named(LiteralEqualityRegime::GENERATOR_NAME));
    let semantics = LiteralEqualitySemantics;

    let result = audit_step(step, context, &registry, &semantics);
    match result {
        AuditResult::Audited(audited_step) => {
            assert_eq!(
                audited_step.audited.outcome,
                Outcome::Audited,
                "the upgraded judgement's outcome must be Audited"
            );
            assert_eq!(
                audited_step.link.target,
                audited_step.derived_id.digest(),
                "the Dependency link must target the Derived judgement's digest"
            );
            assert!(
                audited_step.verified.is_replay_verified(),
                "the checker must flip the decomposition to ReplayVerified"
            );
        }
        AuditResult::Unknown(reason) => {
            panic!("expected the audit-factorization checker to verify and upgrade to Audited, got Unknown({reason})")
        }
    }
}
