//! THE O(Δ) benchmark gate (ADR-0002 §9.1 "THE invariant", §9.3 engineering
//! build order; `spec/Build_Plan_v3_SOC.md` Step 6, E5) — **armed**.
//!
//! ADR-0002 §9.1, verbatim:
//!
//! > **Cost per committed step MUST be ∝ |Δ| × (index fanout), and MUST NOT
//! > be ∝ |world|.** Doubling the number of *inert* configurations MUST NOT
//! > change per-step cost.
//!
//! Measured **deterministically** via the [`soc_core::CostRecord`] work-unit
//! counts the engines emit — never wall-clock timing (flaky under CI load;
//! there is no `criterion` in the Ring-0 whitelist and none is added). Two
//! engines are contrasted over the *same* workload:
//!
//! - the **naive reference oracle**
//!   ([`soc_core::engine::naive_view_over_instrumented`]) recomputes the whole
//!   candidate view from scratch every step — `∝ |world|`, so doubling inert
//!   configurations doubles its cost. It is retained deliberately (ADR-0002
//!   §9.2) and **expected to fail** the invariant; [`naive_oracle_is_world_
//!   proportional_expected_fail`] pins that, proving the harness can tell
//!   world-proportional cost from flat cost.
//! - the **incremental engine** ([`soc_core::IncrementalEngine`]) routes a
//!   world [`Delta`] through its footprint index to only the regimes it
//!   touches — `∝ |Δ| × fanout`, independent of the inert remainder.
//!   [`o_delta_gate_incremental_engine_is_flat`] is the real gate: it is
//!   **no longer `#[ignore]`d** because the incremental engine genuinely
//!   passes it (per the honesty discipline in the Step 6 brief and ADR-0002
//!   §9.3 — only un-ignore a gate that actually holds on the real engine).
//!
//! # Fixture shape — inert *configurations*, per §9.1's exact wording
//!
//! One **active** configuration is registered to the one active regime (its
//! footprint is `{active}`). The **inert** configurations are handles present
//! in the world but registered to **no** regime — no footprint touches them,
//! none is ever reachable as a successor. This mirrors §9.1's own variable
//! ("the number of inert configurations") directly, rather than standing them
//! in as inert *regimes*: the incremental engine's whole job is to not pay for
//! them, and the naive oracle's defining flaw is that it does.

use std::collections::BTreeSet;

use brix_canon::{Digest, Domain};
use soc_core::adm::AdmAll;
use soc_core::delta::{CandidateDelta, Delta, Footprint};
use soc_core::engine::{naive_view_over_instrumented, IncrementalEngine, IncrementalWitnessIndex};
use soc_core::exec::ExecConfig;
use soc_core::intern::{Handle, Interner};
use soc_core::witness_provider::{Candidate, WitnessProvider};

/// The single active regime: sensitive to exactly one configuration
/// `active`, for which it proposes one reflexive candidate `active → active`.
/// Implements **both** [`WitnessProvider`] (the naive baseline recomputes through it)
/// and [`IncrementalWitnessIndex`] (the engine routes deltas through it), from one
/// candidate definition — so the two engines are measured over identical
/// semantics, not two hand-written approximations.
#[derive(Clone, Copy)]
struct ActiveRegime {
    active: Handle,
    witness: Handle,
}

impl ActiveRegime {
    fn candidate(&self) -> Candidate {
        Candidate {
            witness: self.witness,
            successor: self.active,
        }
    }
}

impl WitnessProvider for ActiveRegime {
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        if e.world == self.active {
            vec![self.candidate()]
        } else {
            Vec::new()
        }
    }
}

impl IncrementalWitnessIndex for ActiveRegime {
    fn footprint(&self) -> Footprint {
        Footprint::configs([self.active])
    }

    fn apply(&mut self, delta: &Delta) -> CandidateDelta {
        let mut cd = CandidateDelta::new();
        if delta.added.contains(&self.active) {
            cd.added.insert(self.candidate());
        }
        if delta.removed.contains(&self.active) {
            cd.removed.insert(self.candidate());
        }
        cd
    }
}

fn tag(i: &mut Interner, s: &str) -> Handle {
    i.intern(Digest::of(Domain::Value, s.as_bytes()))
}

/// The fixture: the active regime, its active config handle, `n` inert
/// configuration handles, and the fixed policy/history the naive baseline
/// uses. The inert handles are registered to nothing.
struct Fixture {
    active_regime: ActiveRegime,
    active: Handle,
    inert: BTreeSet<Handle>,
    policy: Handle,
    history: Digest,
}

fn build_fixture(n_inert: usize) -> Fixture {
    let mut i = Interner::new();
    let active = tag(&mut i, "o_delta.active");
    let witness = tag(&mut i, "o_delta.witness");
    let _presentation_handle = tag(&mut i, "o_delta.regime.active");
    let policy = tag(&mut i, "o_delta.policy");
    let history = Digest::of(Domain::Value, b"o_delta.h0");
    let inert = (0..n_inert)
        .map(|k| tag(&mut i, &format!("o_delta.inert.{k}")))
        .collect();
    Fixture {
        active_regime: ActiveRegime { active, witness },
        active,
        inert,
        policy,
        history,
    }
}

/// The **naive oracle's** per-step cost: recompute the whole candidate view
/// from scratch over the present configuration set (`active` ∪ all `inert`).
/// Scans every present config against every regime — `∝ |world|`.
fn measure_naive(f: &Fixture) -> u64 {
    let mut present = f.inert.clone();
    present.insert(f.active);
    let regimes: Vec<&dyn WitnessProvider> = vec![&f.active_regime];
    let (_view, cost) =
        naive_view_over_instrumented(&regimes, &AdmAll, &present, f.policy, f.history);
    cost.work_units()
        .expect("the instrumented naive recompute always measures, never UnknownCost")
}

/// The **incremental engine's** per-step cost: after ingesting every inert
/// configuration as setup (each a footprint-missing no-op — proving the
/// engine genuinely holds a world of `|inert|` present configs), measure the
/// cost of the one committed step that admits the active config. That step's
/// cost is `∝ |Δ| × fanout` and references neither `|inert|` nor the view
/// size — which is the whole invariant.
fn measure_incremental(f: &Fixture) -> u64 {
    let mut engine = IncrementalEngine::new(vec![Box::new(f.active_regime)]);

    // Setup (not measured): the world already contains |inert| configs. Each
    // arrival routes through the footprint index to zero regimes — the engine
    // never accumulates per-inert state, so this is O(1) per config.
    for &h in &f.inert {
        let report = engine.step(&Delta::of_added([h]));
        assert!(
            report.candidate_delta.is_empty(),
            "an inert configuration must induce no candidate change"
        );
    }

    // The measured committed step: the active config enters the world.
    let report = engine.step(&Delta::of_added([f.active]));
    assert_eq!(
        report.candidate_delta.added.len(),
        1,
        "the active config's arrival must induce exactly one candidate"
    );
    report
        .cost
        .work_units()
        .expect("the incremental step always measures, never UnknownCost")
}

/// Whether the O(Δ) invariant HELD between two measurements: the
/// after-doubling cost stayed within `tolerance` work units of before, rather
/// than growing with the (doubled) inert population.
fn o_delta_holds(cost_before: u64, cost_after_doubling_inert: u64, tolerance: u64) -> bool {
    cost_after_doubling_inert <= cost_before.saturating_add(tolerance)
}

/// Small fixed slack: an O(Δ)-conformant engine's cost may vary by a small
/// constant across runs (e.g. a differently-shaped index fanout for the same
/// delta) without that being a regression; growth *with the inert population*
/// is what must not happen.
const TOLERANCE: u64 = 4;

/// How many inert configurations the fixture starts with; the "doubling"
/// measurement re-runs with `2 * N_INERT`.
const N_INERT: usize = 64;

#[test]
fn fixture_is_non_vacuous_active_regime_produces_exactly_one_candidate() {
    let f = build_fixture(3);
    let e = ExecConfig::new(f.active, f.policy, f.history);
    assert_eq!(
        f.active_regime.candidates(&e).len(),
        1,
        "the active regime must produce exactly one candidate at its active config"
    );
    // And nothing at an inert config.
    let inert0 = *f.inert.iter().next().unwrap();
    let e_inert = ExecConfig::new(inert0, f.policy, f.history);
    assert!(f.active_regime.candidates(&e_inert).is_empty());
}

/// **Expected to hold (green, un-ignored).** The naive recompute-the-world
/// oracle is *deliberately* `∝ |world|`, not `∝ |Δ|` (ADR-0002 §9.1/§9.2).
/// Doubling the inert configuration count must measurably grow its per-step
/// cost. If this ever stopped holding, the harness would have gone blind to
/// world-proportional cost — a harness bug, not good news about the oracle.
#[test]
fn naive_oracle_is_world_proportional_expected_fail() {
    let cost_before = measure_naive(&build_fixture(N_INERT));
    let cost_after = measure_naive(&build_fixture(N_INERT * 2));

    assert!(
        cost_after > cost_before,
        "fixture must be non-vacuous: doubling inert configurations must increase the naive \
         recompute's work units (before={cost_before}, after={cost_after})"
    );
    assert!(
        !o_delta_holds(cost_before, cost_after, TOLERANCE),
        "EXPECTED naive-oracle failure did not occur: doubling inert configurations (from \
         {N_INERT} to {}) left the naive recompute's cost within tolerance ({cost_before} -> \
         {cost_after}, tolerance={TOLERANCE}). Per ADR-0002 §9.1/§9.2 the naive \
         recompute-the-world oracle is supposed to fail this invariant by design — if it \
         didn't, the harness can no longer discriminate world-proportional cost from flat \
         cost, which is a bug in the harness, not good news about the oracle.",
        N_INERT * 2
    );
}

/// **THE O(Δ) gate — armed and green on the real incremental engine.**
///
/// Doubling the inert configuration count from `N_INERT` to `2 * N_INERT`
/// leaves the incremental engine's per-committed-step cost **flat** (within
/// `TOLERANCE`): the footprint index routes the active config's arrival to
/// only the active regime, never touching the inert remainder. Per ADR-0002
/// §9.1 this is the anti-v1 invariant, and per §9.3 no further optimization
/// (E6 onward) lands before it is green — which, as of this change, it is.
#[test]
fn o_delta_gate_incremental_engine_is_flat() {
    let cost_before = measure_incremental(&build_fixture(N_INERT));
    let cost_after = measure_incremental(&build_fixture(N_INERT * 2));

    assert!(
        o_delta_holds(cost_before, cost_after, TOLERANCE),
        "O(Δ) gate FAILED: doubling inert configurations changed the incremental engine's \
         per-step cost ({cost_before} -> {cost_after}, tolerance={TOLERANCE}). Per ADR-0002 \
         §9.1 this is a red build — cost per committed step must be independent of inert \
         world size."
    );
}

/// A sharper, quantitative companion to the gate: across a 1×/2×/4×/8× sweep
/// of inert configurations, the incremental engine's per-step cost is
/// *exactly identical* every time (not merely within tolerance), while the
/// naive oracle's grows monotonically. This is the executable statement of
/// "`∝ |Δ|` and NOT `∝ |world|`," leaving no room for a within-tolerance
/// coincidence to mask a slow leak.
#[test]
fn incremental_cost_is_constant_while_naive_grows_across_a_scaling_sweep() {
    let scales = [N_INERT, N_INERT * 2, N_INERT * 4, N_INERT * 8];

    let incremental: Vec<u64> = scales
        .iter()
        .map(|&n| measure_incremental(&build_fixture(n)))
        .collect();
    let naive: Vec<u64> = scales
        .iter()
        .map(|&n| measure_naive(&build_fixture(n)))
        .collect();

    // Incremental: byte-for-byte constant across the whole sweep.
    for w in incremental.windows(2) {
        assert_eq!(
            w[0], w[1],
            "incremental per-step cost must be exactly constant across the inert-config sweep, \
             got {incremental:?}"
        );
    }

    // Naive: strictly increasing across the whole sweep.
    for w in naive.windows(2) {
        assert!(
            w[1] > w[0],
            "naive recompute cost must strictly grow with inert configurations, got {naive:?}"
        );
    }
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
