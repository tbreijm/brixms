//! The O(Δ) benchmark/gate harness (ADR-0002 §9.1 "THE invariant", §9.3
//! engineering build order; `spec/Build_Plan_v3_SOC.md` Step 6, E5;
//! `spec/Next_Steps.md` action 5).
//!
//! ADR-0002 §9.1, verbatim:
//!
//! > **Cost per committed step MUST be ∝ |Δ| × (index fanout), and MUST NOT
//! > be ∝ |world|.** Doubling the number of *inert* configurations MUST NOT
//! > change per-step cost.
//!
//! This is measured **deterministically**, via the [`soc_core::CostRecord`]
//! work-unit counts [`soc_core::cand_instrumented`] emits — never wall-clock
//! timing. Wall-clock benchmarking is flaky under CI load and the workspace
//! Ring-0 dependency whitelist (root `Cargo.toml`) has no `criterion` entry;
//! none is added here or anywhere in this change.
//!
//! # Fixture shape
//!
//! One **active** [`Regime`] actually participates: for the fixture's `e0`
//! it proposes exactly one candidate witness. Any number of **inert**
//! regimes are pure ballast: each is asked for its candidates on every
//! `cand`/`cand_instrumented` call (the naive oracle scans *every* regime in
//! its `regimes: &[&dyn Regime]` slice, unconditionally — that is its
//! O(|world|) shape), and each always returns nothing. An inert regime here
//! stands in for one inert configuration present in the world/store but
//! contributing no candidates and never reachable as a successor.
//!
//! # The expected-fail / armed-future-gate split (read before touching this
//! file)
//!
//! Per ADR-0002 §9.2 "Reference oracle" and §9.3's build order, the naive
//! `soc-core` oracle in this crate is *deliberately* the anti-v1 reference:
//! it recomputes the world on every call and is correct, not fast. It is
//! **expected, by design**, to fail the O(Δ) invariant — doubling inert
//! regimes doubles the number of regimes it unconditionally scans, hence
//! doubles measured work units. That is not a bug in the oracle; "optimize
//! the naive oracle" is explicitly forbidden (ADR-0002 §9.2). So this file
//! carries two tests with two different jobs:
//!
//! - [`naive_oracle_is_world_proportional_expected_fail`] — asserts the
//!   naive oracle **fails** `o_delta_holds`. This is a green, un-ignored
//!   test: it proves the *harness itself* discriminates world-proportional
//!   cost from flat cost (if this test ever started passing — i.e.
//!   `o_delta_holds` returning `true` for the naive oracle — the harness
//!   would have gone blind, which is the real regression to worry about).
//! - [`o_delta_gate_for_incremental_engine`] — the **real** future gate,
//!   `#[ignore]`d for now. It is the literal executable acceptance
//!   criterion for Build Plan v3 Step 6's E5: when the delta-driven
//!   incremental engine (E3/E4) lands, wire its instrumented candidate
//!   computation in here (replacing/alongside `cand_instrumented`), remove
//!   the `#[ignore]`, and this test going green *is* the E5 gate closing.
//!   Per ADR-0002 §9.3, no further optimization may land before it does.

use brix_canon::{Digest, Domain};
use soc_core::adm::AdmAll;
use soc_core::exec::ExecConfig;
use soc_core::history::History;
use soc_core::intern::{Handle, Interner};
use soc_core::oracle::cand_instrumented;
use soc_core::regime::{Candidate, Regime};

/// The single active regime in the fixture: proposes exactly one candidate
/// witness when queried at `from`, nothing otherwise. Mirrors
/// `tests/governance_conservation.rs`'s `FixtureRegime` shape, scoped down
/// to a single edge — the O(Δ) gate only needs "this regime does real
/// work," not multi-edge graph traversal.
struct ActiveRegime {
    id: Handle,
    from: Handle,
    witness: Handle,
    to: Handle,
}

impl Regime for ActiveRegime {
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        if e.world == self.from {
            vec![Candidate {
                regime: self.id,
                witness: self.witness,
                successor: self.to,
            }]
        } else {
            Vec::new()
        }
    }
}

/// Pure ballast: an inert configuration. Scanned by the naive oracle's
/// `cand`/`cand_instrumented` loop on every call (it is unconditionally
/// present in the `regimes` slice) but never produces a candidate and is
/// never reachable as a successor of anything.
struct InertRegime;

impl Regime for InertRegime {
    fn candidates(&self, _e: &ExecConfig) -> Vec<Candidate> {
        Vec::new()
    }
}

fn tag(i: &mut Interner, s: &str) -> Handle {
    i.intern(Digest::of(Domain::Value, s.as_bytes()))
}

/// Build the fixture's one active regime plus its exec config `e0`.
fn build_active(i: &mut Interner) -> (ActiveRegime, ExecConfig) {
    let from = tag(i, "o_delta.w0");
    let to = tag(i, "o_delta.w1");
    let regime = tag(i, "o_delta.regime.active");
    let witness = tag(i, "o_delta.witness");
    let policy = tag(i, "o_delta.policy0");
    let e0 = ExecConfig::new(from, policy, History::empty().digest());
    (
        ActiveRegime {
            id: regime,
            from,
            witness,
            to,
        },
        e0,
    )
}

/// `n` inert ballast regimes — indistinguishable from one another by
/// design; only their count matters for the invariant under test.
fn build_inert(n: usize) -> Vec<InertRegime> {
    (0..n).map(|_| InertRegime).collect()
}

/// Measure the per-step cost (in deterministic work units) of one
/// `cand_instrumented` call over `active` plus every regime in `inert`.
fn measure(active: &ActiveRegime, inert: &[InertRegime], e: &ExecConfig) -> u64 {
    let mut regimes: Vec<&dyn Regime> = Vec::with_capacity(1 + inert.len());
    regimes.push(active);
    for r in inert {
        regimes.push(r);
    }
    let (_candidates, cost) = cand_instrumented(&regimes, &AdmAll, e);
    cost.work_units()
        .expect("cand_instrumented always emits a measured Steps cost, never UnknownCost")
}

/// Per ADR-0002 §9.1: doubling inert configurations must not change
/// per-step cost. Returns whether the O(Δ) invariant HELD between the two
/// measurements — i.e. `cost_after_doubling_inert` stayed within
/// `tolerance` work units of `cost_before` rather than growing with the
/// (doubled) inert population.
fn o_delta_holds(cost_before: u64, cost_after_doubling_inert: u64, tolerance: u64) -> bool {
    cost_after_doubling_inert <= cost_before.saturating_add(tolerance)
}

/// Small fixed slack for `o_delta_holds`'s tolerance: an O(Δ)-conformant
/// engine's cost may vary by a small constant across runs (e.g. a
/// differently-shaped index fanout for the same delta) without that being a
/// regression; growth *with the inert population* is what must not happen.
const TOLERANCE: u64 = 4;

/// How many inert configurations the fixture starts with; the "doubling"
/// measurement re-runs with `2 * N_INERT`.
const N_INERT: usize = 64;

#[test]
fn fixture_is_non_vacuous_active_regime_produces_exactly_one_candidate() {
    let mut i = Interner::new();
    let (active, e0) = build_active(&mut i);
    let inert = build_inert(3);
    let mut regimes: Vec<&dyn Regime> = vec![&active];
    for r in &inert {
        regimes.push(r);
    }
    let (candidates, _cost) = cand_instrumented(&regimes, &AdmAll, &e0);
    assert_eq!(
        candidates.len(),
        1,
        "fixture must be non-vacuous: the active regime must produce exactly \
         one candidate, and the inert regimes none"
    );
}

/// **This test is expected to hold (green, un-ignored).** The naive
/// recompute-the-world oracle in this crate (`soc_core::oracle`) is
/// *deliberately* O(|world|), not O(Δ) — see the module doc comment and
/// ADR-0002 §9.1/§9.2/§9.3. Doubling the inert configuration count from
/// `N_INERT` to `2 * N_INERT` must measurably change (grow) per-step cost,
/// because the naive oracle scans every regime — active or inert —
/// unconditionally on every call.
///
/// If this test ever starts *passing the invariant* (i.e. `o_delta_holds`
/// returns `true` here), that is not good news: it means the harness has
/// gone blind to world-proportional cost, which is a bug in the harness
/// itself, not evidence the naive oracle became O(Δ). Investigate
/// `cand_instrumented`'s work-unit accounting first.
#[test]
fn naive_oracle_is_world_proportional_expected_fail() {
    let mut i = Interner::new();
    let (active, e0) = build_active(&mut i);
    let inert_n = build_inert(N_INERT);
    let inert_2n = build_inert(N_INERT * 2);

    let cost_before = measure(&active, &inert_n, &e0);
    let cost_after = measure(&active, &inert_2n, &e0);

    assert!(
        cost_after > cost_before,
        "fixture must be non-vacuous: doubling inert regimes must actually \
         increase measured work units (before={cost_before}, after={cost_after})"
    );
    assert!(
        !o_delta_holds(cost_before, cost_after, TOLERANCE),
        "EXPECTED naive-oracle failure did not occur: doubling inert \
         configurations (from {N_INERT} to {}) left per-step cost within \
         tolerance ({cost_before} -> {cost_after}, tolerance={TOLERANCE}). \
         Per ADR-0002 §9.1/§9.2 the naive recompute-the-world oracle is \
         supposed to fail this invariant by design — if it didn't, the \
         harness can no longer discriminate world-proportional cost from \
         flat cost, which is a bug in the harness, not good news about the \
         oracle.",
        N_INERT * 2
    );
}

/// **The real O(Δ) gate — armed for the incremental engine (E4), not yet
/// runnable.** `#[ignore]`d because the only oracle this crate has today is
/// the naive one, and per ADR-0002 §9.2 it must never be "optimized" to
/// pass this — a faster engine belongs *beside* it (E3/E4), not instead of
/// it. This test's body is the literal acceptance criterion for
/// `Build_Plan_v3_SOC.md` Step 6 / ADR-0002 §9.3's E5: when the
/// delta-driven incremental engine lands, point its own instrumented
/// candidate computation at this fixture (replacing the
/// `soc_core::oracle::cand_instrumented` call in `measure` with the
/// incremental engine's equivalent, differentially tested against the
/// naive oracle per ADR-0002 §9.2), remove `#[ignore]`, and this test
/// going green **is** E5 closing. Per ADR-0002 §9.3, no further
/// optimization (E6 onward) may land before it does.
#[test]
#[ignore = "arms the O(Δ) gate for the incremental engine (E4); the naive oracle fails it by design (ADR-0002 §9.1)"]
fn o_delta_gate_for_incremental_engine() {
    let mut i = Interner::new();
    let (active, e0) = build_active(&mut i);
    let inert_n = build_inert(N_INERT);
    let inert_2n = build_inert(N_INERT * 2);

    let cost_before = measure(&active, &inert_n, &e0);
    let cost_after = measure(&active, &inert_2n, &e0);

    assert!(
        o_delta_holds(cost_before, cost_after, TOLERANCE),
        "O(Δ) gate FAILED: doubling inert configurations changed per-step \
         cost ({cost_before} -> {cost_after}, tolerance={TOLERANCE}). Per \
         ADR-0002 §9.1 this is a red build for the incremental engine (E4) \
         — cost per committed step must be independent of inert world size."
    );
}

#[cfg(test)]
mod o_delta_holds_unit_tests {
    use super::o_delta_holds;

    #[test]
    fn holds_when_cost_is_unchanged() {
        assert!(o_delta_holds(100, 100, 0));
    }

    #[test]
    fn holds_within_tolerance() {
        assert!(o_delta_holds(100, 102, 4));
    }

    #[test]
    fn fails_outside_tolerance() {
        assert!(!o_delta_holds(100, 200, 4));
    }

    #[test]
    fn holds_when_cost_decreases() {
        assert!(o_delta_holds(100, 50, 0));
    }
}
