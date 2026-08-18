//! The fallible saturated commit path (#254, ADR-0012 §4.3 step 5 / §6.3 /
//! §9 Stage C fixtures 7 and 8).
//!
//! `SaturationUnknown::{KeyConflict, CommitFailed}` were declared but
//! structurally unreachable: `sat_step` drove the reference `commit_tick`,
//! which panics on both conditions. Stage C's fixtures exercised the fallible
//! *primitives* (`Frontier::apply_delta`, `try_commit_selected`) directly,
//! which proved the primitives fail closed but not that **a run** does.
//!
//! These tests close that gap. Each drives `run_saturated` end to end with an
//! injected fault and asserts the run stops closed: no panic, no committed
//! step, no certificate, and — for the commit boundary — the underlying
//! `CommitError` preserved across the trip.
//!
//! A dead failure mode reads as a handled failure mode. These are what make
//! the difference observable.

use std::collections::BTreeMap;

use brix_canon::{CanonWriter, Digest, Domain};
use brix_semantic::{ConfigId, ContextId, Decomposition, GeneratorId};
use soc_core::adm::AdmAll;
use soc_core::calendar::Key;
use soc_core::commit::{CommitError, SettlementWitnessProvider};
use soc_core::exec::ExecConfig;
use soc_core::history::History;
use soc_core::intern::{Handle, Interner};
use soc_core::saturate::{
    run_saturated, DeclaredAssumptions, GeneratorPartitionProfile, ObservationProfile,
    PresentationIdV1, PresentationV1, SaturatedStop, SaturationBudget, SaturationUnknown,
};
use soc_core::witness_provider::{Candidate, WitnessProvider};

// ---------------------------------------------------------------------------
// Fixture: one world with a configurable number of outgoing candidates, and a
// `try_decompose` that can be told to reject.
// ---------------------------------------------------------------------------

struct FaultyRegime {
    /// Every candidate offered at `origin`, in order. Two distinct successors
    /// are what a colliding keyer needs in order to actually conflict — one
    /// candidate keyed twice is idempotent, not a conflict.
    edges: Vec<(Handle, Handle)>,
    origin: Handle,
    configs: BTreeMap<Handle, ConfigId>,
    /// When `Some`, `try_decompose` rejects with this instead of building a
    /// decomposition — the source-derived regime failure of ADR-0012 §6.3.
    decompose_error: Option<CommitError>,
}

impl WitnessProvider for FaultyRegime {
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        if e.world != self.origin {
            return Vec::new();
        }
        self.edges
            .iter()
            .map(|(witness, successor)| Candidate {
                witness: *witness,
                successor: *successor,
            })
            .collect()
    }
}

impl SettlementWitnessProvider for FaultyRegime {
    fn try_decompose(&self, e: &ExecConfig, c: &Candidate) -> Result<Decomposition, CommitError> {
        if let Some(error) = &self.decompose_error {
            return Err(error.clone());
        }
        Ok(Decomposition::recorded(
            vec![gen_realizing()],
            vec![self.configs[&e.world], self.configs[&c.successor]],
        )
        .expect("well-formed decomposition"))
    }
}

fn tag(i: &mut Interner, s: &str) -> Handle {
    i.intern(Digest::of(Domain::Value, s.as_bytes()))
}

fn gen_realizing() -> GeneratorId {
    GeneratorId::named("fallible-fixture.realizing@1")
}

fn profile() -> GeneratorPartitionProfile {
    GeneratorPartitionProfile::new(Default::default(), [gen_realizing()].into_iter().collect())
        .expect("disjoint partitions")
}

struct Fixture {
    interner: Interner,
    regime: FaultyRegime,
    origin: Handle,
    policy: Handle,
}

/// `successors` worlds reachable from `w0`; `decompose_error` optionally makes
/// the commit boundary reject.
fn fixture(successors: usize, decompose_error: Option<CommitError>) -> Fixture {
    let mut interner = Interner::new();
    let origin = tag(&mut interner, "w0");
    let policy = tag(&mut interner, "fallible.policy");
    let _presentation_handle = tag(&mut interner, "fallible.regime");

    let mut edges = Vec::new();
    let mut worlds = vec![origin];
    for n in 0..successors {
        let successor = tag(&mut interner, &format!("w{}", n + 1));
        let witness = tag(&mut interner, &format!("fallible.witness.{n}"));
        worlds.push(successor);
        edges.push((witness, successor));
    }

    let configs = worlds
        .iter()
        .map(|h| (*h, ConfigId(interner.resolve(*h))))
        .collect();

    Fixture {
        regime: FaultyRegime {
            edges,
            origin,
            configs,
            decompose_error,
        },
        interner,
        origin,
        policy,
    }
}

impl Fixture {
    fn exec(&self) -> ExecConfig {
        ExecConfig::new(self.origin, self.policy, History::empty().digest())
    }
}

fn presentation<'a>(
    regimes: &'a [&'a dyn SettlementWitnessProvider],
    profile: &'a dyn ObservationProfile,
    interner: &'a Interner,
) -> PresentationV1<'a> {
    PresentationV1 {
        id: PresentationIdV1::from_canon(b"fallible-fixture@1"),
        regimes,
        regime_set: Digest::of(Domain::Value, b"fallible.regime-set"),
        adm: &AdmAll,
        adm_id: Digest::of(Domain::Value, b"fallible.adm-all"),
        profile,
        interner,
        context: ContextId::root(),
        assumptions: DeclaredAssumptions::all(),
    }
}

/// The honest keyer: a tie-break unique per distinct candidate.
fn unique_keyer() -> impl FnMut(&Candidate, u64) -> Key {
    |c: &Candidate, phase: u64| {
        let mut w = CanonWriter::new();
        w.write_uint(c.witness.raw() as u64);
        w.write_uint(c.successor.raw() as u64);
        Key::new(phase, 0, w.digest(Domain::Value))
    }
}

/// A keyer whose tie-break is **not** unique — every candidate lands on the
/// same key. This is the source-derived keyer bug ADR-0012 §6.3 requires the
/// run to survive without panicking.
fn colliding_keyer() -> impl FnMut(&Candidate, u64) -> Key {
    |_c: &Candidate, phase: u64| Key::new(phase, 0, Digest::of(Domain::Value, b"same-key"))
}

fn budget() -> SaturationBudget {
    SaturationBudget::uniform(64)
}

// ---------------------------------------------------------------------------
// Acceptance
// ---------------------------------------------------------------------------

#[test]
fn a_key_conflict_during_a_run_is_unknown_never_a_panic() {
    // Two candidates with different observed successors, one key: the B^uk
    // discipline is violated. The reference driver panics here; a run must not.
    let f = fixture(2, None);
    let regime: &dyn SettlementWitnessProvider = &f.regime;
    let profile = profile();
    let pres = presentation(std::slice::from_ref(&regime), &profile, &f.interner);

    let mut k = colliding_keyer();
    let run = run_saturated(&pres, f.exec(), &mut k, budget());

    match run.stop {
        SaturatedStop::Unknown(SaturationUnknown::KeyConflict { at_step }) => {
            assert_eq!(at_step, 0, "the conflict is at the first step of the run");
        }
        SaturatedStop::Quiescent(_) => {
            panic!("a key conflict must never be reported as certified quiescence")
        }
        other => panic!("expected Unknown(KeyConflict), got {other:?}"),
    }

    assert!(
        run.journal.is_empty(),
        "a conflicted tick must commit no step"
    );
    assert!(
        run.visible.is_empty(),
        "a conflicted tick must export no visible observation"
    );
}

#[test]
fn a_commit_boundary_failure_during_a_run_is_unknown_and_preserves_the_error() {
    // The Stage B vocabulary must survive the trip: a regime that rejects its
    // own candidate is `GeneratorMismatch`, and the run must say so rather
    // than flattening it to a generic failure.
    let f = fixture(1, Some(CommitError::GeneratorMismatch));
    let regime: &dyn SettlementWitnessProvider = &f.regime;
    let profile = profile();
    let pres = presentation(std::slice::from_ref(&regime), &profile, &f.interner);

    let mut k = unique_keyer();
    let run = run_saturated(&pres, f.exec(), &mut k, budget());

    match run.stop {
        SaturatedStop::Unknown(SaturationUnknown::CommitFailed { at_step, error }) => {
            assert_eq!(at_step, 0);
            assert_eq!(
                error,
                CommitError::GeneratorMismatch,
                "the underlying CommitError must survive the trip to the stop vocabulary"
            );
        }
        SaturatedStop::Quiescent(_) => {
            panic!("a failed commit boundary must never be reported as certified quiescence")
        }
        other => panic!("expected Unknown(CommitFailed), got {other:?}"),
    }

    assert!(
        run.journal.is_empty(),
        "a rejected commit boundary must commit no step"
    );
}

#[test]
fn every_stage_b_commit_error_survives_the_trip() {
    // ADR-0012 §6.3's four conditions each have their own variant precisely so
    // a diagnostic never misdescribes which check failed; flattening them here
    // would undo that.
    for expected in [
        CommitError::UnresolvedHandle,
        CommitError::EmptyDecomposition,
        CommitError::EndpointMismatch,
        CommitError::CandidateMismatch,
        CommitError::WitnessMismatch,
        CommitError::GeneratorMismatch,
        CommitError::ChainLengthMismatch {
            generators: 2,
            configs: 2,
        },
    ] {
        let f = fixture(1, Some(expected.clone()));
        let regime: &dyn SettlementWitnessProvider = &f.regime;
        let profile = profile();
        let pres = presentation(std::slice::from_ref(&regime), &profile, &f.interner);

        let mut k = unique_keyer();
        let run = run_saturated(&pres, f.exec(), &mut k, budget());

        match run.stop {
            SaturatedStop::Unknown(SaturationUnknown::CommitFailed { error, .. }) => {
                assert_eq!(error, expected, "commit error was not preserved");
            }
            other => panic!("expected Unknown(CommitFailed({expected:?})), got {other:?}"),
        }
    }
}

#[test]
fn an_honest_run_still_reaches_quiescence() {
    // The fence refuses only faulty runs. Without an injected fault the same
    // fixture settles: one realizing step, then certified quiescence.
    let f = fixture(1, None);
    let regime: &dyn SettlementWitnessProvider = &f.regime;
    let profile = profile();
    let pres = presentation(std::slice::from_ref(&regime), &profile, &f.interner);

    let mut k = unique_keyer();
    let run = run_saturated(&pres, f.exec(), &mut k, budget());

    assert!(
        run.stop.is_quiescent(),
        "an unfaulted run must still settle, got {:?}",
        run.stop
    );
    assert_eq!(run.journal.len(), 1, "exactly one committed step");
    assert_eq!(run.visible.len(), 1, "exactly one visible observation");
}

#[test]
fn the_reference_driver_still_panics_on_a_key_conflict() {
    // `commit_tick` keeps its contract (ADR-0012 §2.5): for the reference
    // driver these conditions are internal-consistency bugs, not states a
    // caller handles. That is the whole reason the fallible sibling exists
    // rather than the reference driver simply becoming fallible.
    let f = fixture(2, None);
    let regime: &dyn SettlementWitnessProvider = &f.regime;
    let mut k = colliding_keyer();

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        soc_core::commit::commit_tick(
            std::slice::from_ref(&regime),
            &AdmAll,
            &f.interner,
            &f.exec(),
            ContextId::root(),
            0,
            &mut k,
        )
    }))
    .is_err();

    assert!(
        panicked,
        "the reference driver must still panic on a B^uk violation"
    );
}
