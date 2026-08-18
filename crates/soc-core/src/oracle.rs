//! The naive reference oracle: `cand(e)` and `Succ(e)` (ADR-0002 §3, §9.2
//! "Reference oracle"; `Build_Plan_v3_SOC.md` Step 3, S2⋈E2):
//!
//! > The naive recompute-the-world loop is deliberately reimplemented as the
//! > reference oracle; the fast engine is differential-tested against it
//! > (SOC settlement discipline, made testable).
//!
//! `brix-oracle`'s role reborn at the SOC layer: single-threaded,
//! recompute-the-world, **correct, not fast**. `cand(e)` is the union over
//! providers of their (naive) [`crate::witness_provider::WitnessProvider::candidates`], filtered
//! by an [`crate::adm::Adm`]. `Succ(e)` is the observed successor-config
//! set: every admissible candidate applied to `e`. This is deliberately the
//! differential-test baseline the later incremental engine (`Build_Plan_v3_SOC.md`
//! Step 6, E3/E4) is checked against — never "optimize" this module; a
//! faster engine belongs beside it, not instead of it.

use crate::adm::Adm;
use crate::cost::CostRecord;
use crate::exec::ExecConfig;
use crate::history::History;
use crate::intern::Handle;
use crate::witness_provider::{Candidate, WitnessProvider};
use brix_canon::{CanonWriter, Canonical, Digest};
use std::collections::BTreeSet;

/// `cand(e)` = the union over `providers` of `provider.candidates(e)`, filtered
/// by `adm`. A `BTreeSet` (Ring0 §0 determinism discipline — no `HashSet`)
/// so two calls with identical inputs return byte-identical results in the
/// same iteration order.
pub fn cand(
    providers: &[&dyn WitnessProvider],
    adm: &dyn Adm,
    e: &ExecConfig,
) -> BTreeSet<Candidate> {
    let mut out = BTreeSet::new();
    for provider in providers {
        for c in provider.candidates(e) {
            if adm.admits(e, &c) {
                out.insert(c);
            }
        }
    }
    out
}

/// An instrumented variant of [`cand`] that additionally emits a
/// [`CostRecord`] measuring the work this call actually did (ADR-0001 §4
/// stage-4a; ADR-0002 §9.1, the O(Δ) gate — `tests/o_delta_gate.rs`). Does
/// **not** change `cand`'s behavior: same filtering, same resulting
/// candidate set, byte-for-byte. This is a pure, non-invasive addition
/// beside the reference oracle, per ADR-0002 §9.2 "Reference oracle"
/// discipline (never optimize or alter the naive oracle's semantics; add
/// instrumentation beside it, not instead of it).
///
/// **Why this is exactly the naive oracle's real cost.** `cand` asks
/// *every* provider in `providers` for its candidates on *every* call,
/// unconditionally — whether that provider is active (produces a candidate
/// for this `e`) or inert (produces nothing). That unconditional scan is
/// the O(|world|) "recompute-the-world" cost ADR-0002 §9.1's O(Δ) gate
/// exists to catch. Work units here: one per provider scanned
/// (`providers.len()` scans, paid regardless of relevance) plus one per raw
/// candidate a provider returns, scanned for admissibility before any
/// admissible ones are inserted into the output set. Doubling the number of
/// *inert* providers in `providers` therefore directly doubles the work-unit
/// count measured here — that is what makes the naive oracle's
/// world-proportional cost observable at all (see `tests/o_delta_gate.rs`).
pub fn cand_instrumented(
    providers: &[&dyn WitnessProvider],
    adm: &dyn Adm,
    e: &ExecConfig,
) -> (BTreeSet<Candidate>, CostRecord) {
    let mut out = BTreeSet::new();
    let mut work: u64 = 0;
    for provider in providers {
        // One work unit per provider scanned, paid unconditionally — this is
        // the naive oracle's O(|world|) shape, made measurable.
        work += 1;
        for c in provider.candidates(e) {
            // One work unit per raw candidate scanned for admissibility,
            // whether or not it ends up admitted.
            work += 1;
            if adm.admits(e, &c) {
                out.insert(c);
            }
        }
    }
    (out, CostRecord::Steps(work))
}

/// `Succ(e)` — the observed successor-config set: every [`ExecConfig`]
/// reached by applying an admissible candidate from `cand(e)`. Each
/// candidate's witness+successor pair is folded into the history digest
/// chain ([`History::append`]) to produce the successor's `h` component,
/// exactly the same fold the calendar/commit step (`Build_Plan_v3_SOC.md`
/// Step 4) will use for the *singular committed* step — here, every
/// admissible candidate is "applied" (this is the deliberation frontier
/// `B^uk_{K,O}`, not `select_K`'s singular commit).
///
/// Because each successor's world/policy/history is a pure function of `e`
/// and the candidate alone (never of `adm`), `Succ(e)` is exactly the image
/// of `cand(e)` under that function — which is *why* the governance
/// conservation law (`cand'(e) ⊆ cand(e) ⟹ Succ'(e) ⊆ Succ(e)`) holds
/// structurally here, not merely by the test fixture's luck.
pub fn succ(
    providers: &[&dyn WitnessProvider],
    adm: &dyn Adm,
    e: &ExecConfig,
) -> BTreeSet<ExecConfig> {
    cand(providers, adm, e)
        .into_iter()
        .map(|c| apply(e, &c))
        .collect()
}

/// Apply one candidate to `e`, producing its successor `ExecConfig`. Policy
/// carries over unchanged; the world becomes the candidate's successor
/// handle; the history digest folds in the applied witness.
///
/// `pub(crate)` (not private): [`crate::commit`]'s committed loop reuses this
/// verbatim to advance `e` after `select_K` commits a candidate, so the
/// committed successor's history component folds exactly the same way the
/// naive oracle's deliberation-frontier successors do (ADR-0002 §9.2 —
/// "oracle and committed loop share candidate enumeration"; this extends
/// that sharing to the successor-construction step). This is a visibility
/// change only — `apply`'s behavior is untouched, per the "never optimize or
/// alter the naive oracle" discipline (module docs).
pub(crate) fn apply(e: &ExecConfig, c: &Candidate) -> ExecConfig {
    ExecConfig::new(c.successor, e.policy, next_history(e, c))
}

fn next_history(e: &ExecConfig, c: &Candidate) -> Digest {
    History::from_digest(e.history, 0)
        .append(&CandidateStep {
            witness: c.witness,
            successor: c.successor,
        })
        .digest()
}

/// The canonical payload folded into the history chain when a candidate is
/// applied: the witness and successor handles' raw indices. Stable within a
/// single run of a fixed [`crate::intern::Interner`] — the only context this
/// crate uses it in (see [`Handle::raw`]'s docs).
struct CandidateStep {
    witness: Handle,
    successor: Handle,
}

impl Canonical for CandidateStep {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_record([
            ("successor", (self.successor.raw() as u64).canon_bytes()),
            ("witness", (self.witness.raw() as u64).canon_bytes()),
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adm::{AdmAll, AdmNone};
    use crate::intern::Interner;
    use brix_canon::Domain;

    struct FixedRegime {
        out: Vec<(Handle, Handle)>,
    }

    impl WitnessProvider for FixedRegime {
        fn candidates(&self, _e: &ExecConfig) -> Vec<Candidate> {
            self.out
                .iter()
                .map(|&(witness, successor)| Candidate { witness, successor })
                .collect()
        }
    }

    fn setup() -> (Interner, FixedRegime, ExecConfig) {
        let mut i = Interner::new();
        let world = i.intern(Digest::of(Domain::Value, b"w0"));
        let policy = i.intern(Digest::of(Domain::Value, b"p0"));
        let witness = i.intern(Digest::of(Domain::Value, b"wit"));
        let successor = i.intern(Digest::of(Domain::Value, b"w1"));
        let e = ExecConfig::new(world, policy, History::empty().digest());
        (
            i,
            FixedRegime {
                out: vec![(witness, successor)],
            },
            e,
        )
    }

    #[test]
    fn cand_under_adm_none_is_always_empty() {
        let (_i, provider, e) = setup();
        let providers: Vec<&dyn WitnessProvider> = vec![&provider];
        assert!(cand(&providers, &AdmNone, &e).is_empty());
        assert!(succ(&providers, &AdmNone, &e).is_empty());
    }

    #[test]
    fn cand_under_adm_all_returns_the_regimes_candidates() {
        let (_i, provider, e) = setup();
        let providers: Vec<&dyn WitnessProvider> = vec![&provider];
        let cs = cand(&providers, &AdmAll, &e);
        assert_eq!(cs.len(), 1);
    }

    #[test]
    fn succ_advances_the_world_handle_and_the_history_digest() {
        let (_i, provider, e) = setup();
        let providers: Vec<&dyn WitnessProvider> = vec![&provider];
        let successors = succ(&providers, &AdmAll, &e);
        assert_eq!(successors.len(), 1);
        let s = successors.into_iter().next().unwrap();
        assert_eq!(s.world, provider.out[0].1);
        assert_eq!(s.policy, e.policy);
        assert_ne!(s.history, e.history, "applying a step must advance history");
    }

    #[test]
    fn succ_is_deterministic() {
        let (_i, provider, e) = setup();
        let providers: Vec<&dyn WitnessProvider> = vec![&provider];
        assert_eq!(succ(&providers, &AdmAll, &e), succ(&providers, &AdmAll, &e));
    }

    #[test]
    fn cand_instrumented_matches_cand_s_candidate_set() {
        let (_i, provider, e) = setup();
        let providers: Vec<&dyn WitnessProvider> = vec![&provider];
        let (instrumented, _cost) = cand_instrumented(&providers, &AdmAll, &e);
        assert_eq!(
            instrumented,
            cand(&providers, &AdmAll, &e),
            "cand_instrumented must not change cand's candidate set"
        );
    }

    #[test]
    fn cand_instrumented_never_emits_unknown_cost() {
        let (_i, provider, e) = setup();
        let providers: Vec<&dyn WitnessProvider> = vec![&provider];
        let (_c, cost) = cand_instrumented(&providers, &AdmAll, &e);
        assert!(
            cost.work_units().is_some(),
            "the instrumented oracle path always measures — never UnknownCost"
        );
    }

    #[test]
    fn cand_instrumented_cost_scales_with_the_number_of_regimes_scanned() {
        let (_i, provider, e) = setup();
        let regimes_one: Vec<&dyn WitnessProvider> = vec![&provider];
        let regimes_two: Vec<&dyn WitnessProvider> = vec![&provider, &provider];
        let (_c1, cost1) = cand_instrumented(&regimes_one, &AdmAll, &e);
        let (_c2, cost2) = cand_instrumented(&regimes_two, &AdmAll, &e);
        assert!(
            cost2.work_units().unwrap() > cost1.work_units().unwrap(),
            "scanning more providers must cost strictly more work units \
             (the O(|world|) shape the O(Δ) gate is built to catch)"
        );
    }
}
