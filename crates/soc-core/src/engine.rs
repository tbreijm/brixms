//! The incremental engine (ADR-0002 §9.1 "THE invariant", §9.2 "Candidates
//! are a materialized incremental view"; `Build_Plan_v3_SOC.md` Step 6, E3):
//! the delta-driven counterpart to the naive [`crate::oracle`], landing
//! **beside** it — never replacing it (the naive oracle is the retained
//! reference oracle the fast engine is differential-tested against, ADR-0002
//! §9.2).
//!
//! A regime, as a *running engine component*, is a **dataflow operator**
//! ([`IncrementalRegime`]): it declares a [`Footprint`] and, given a world
//! [`Delta`], returns only the [`CandidateDelta`] it induces — never a re-run
//! of the whole `ρ_w` relation.
//!
//! [`IncrementalEngine`] maintains the materialized candidate **view** as a
//! `BTreeSet<Candidate>` and keeps it in step with a stream of world deltas.
//! The one mechanism that earns the O(Δ) invariant is the **footprint
//! index**: built once at construction from every regime's declared
//! footprint, it lets [`IncrementalEngine::step`] route a delta to *only* the
//! regimes whose footprint intersects it, in `O(|Δ| × fanout)`, never
//! scanning the inert remainder. Constructing the index is a one-time setup
//! cost that may scale with the world; a **committed step** (what §9.1 bounds)
//! never does.
//!
//! **Differential identity (the correctness anchor, ADR-0002 §9.2).** After
//! any sequence of world deltas that presents a configuration set `W`, the
//! engine's [`IncrementalEngine::view`] MUST equal the naive oracle's
//! candidate union recomputed from scratch over `W`
//! ([`naive_view_over`]) — byte-identical `BTreeSet` iteration order. That
//! equality, checked across the conformance corpus, is what earns trust in
//! the fast path; it is exercised in `soc-core`'s own
//! `tests/incremental_differential.rs` and extended over the type corpus in
//! `brix-conformance`.

use std::collections::{BTreeMap, BTreeSet};

use crate::adm::Adm;
use crate::cost::CostRecord;
use crate::delta::{CandidateDelta, Delta, Footprint};
use crate::exec::ExecConfig;
use crate::intern::Handle;
use crate::regime::{Candidate, Regime};

/// A realization regime presented as an incremental **dataflow operator**
/// (ADR-0002 §9.2). The delta-driven counterpart to [`Regime::candidates`]:
/// where the naive [`Regime`] recomputes its *entire* candidate set from an
/// [`ExecConfig`] on every call, an `IncrementalRegime` declares which
/// configurations it is sensitive to ([`footprint`](Self::footprint)) and,
/// given a world [`Delta`], returns only the [`CandidateDelta`] that delta
/// induces ([`apply`](Self::apply)).
///
/// The two traits are deliberately independent (`soc-core` never modifies the
/// naive `Regime`): a regime type may implement both, and the differential
/// gate relies on it — the incremental `apply` stream must reconstruct
/// exactly the naive `candidates` union (see [`naive_view_over`]).
pub trait IncrementalRegime {
    /// The set of configurations this regime is sensitive to. The engine
    /// **skips** this regime entirely for any delta whose footprint does not
    /// intersect — that skip is the O(Δ) invariant (ADR-0002 §9.1). A
    /// footprint must be honest: it MUST include every configuration for
    /// which [`apply`](Self::apply) could produce a non-empty candidate
    /// delta, or the engine will silently drop candidates the naive oracle
    /// keeps (a differential-identity failure).
    fn footprint(&self) -> Footprint;

    /// Consume one world `delta` and return **only** the candidate delta it
    /// induces for this regime. Called by the engine solely when the
    /// regime's [`footprint`](Self::footprint) intersects `delta` — an
    /// implementation may assume (but need not require) that at least one
    /// touched handle is one it cares about.
    fn apply(&mut self, delta: &Delta) -> CandidateDelta;
}

/// One incremental step's result: the combined [`CandidateDelta`] merged into
/// the view this step, plus this step's [`CostRecord`] — the per-committed-step
/// cost ADR-0002 §9.1's O(Δ) gate measures. The cost is always a measured
/// `Steps` count (never `UnknownCost`): the engine knows exactly how much
/// work it did.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StepReport {
    /// The net candidate delta applied to the view this step.
    pub candidate_delta: CandidateDelta,
    /// The deterministic work-unit cost of this step — routing lookups plus
    /// applied regimes plus produced candidate-delta entries. Provably
    /// independent of the inert configuration/regime population (see the
    /// module docs and `tests/o_delta_gate.rs`).
    pub cost: CostRecord,
}

/// The incremental engine (ADR-0002 §9.1/§9.2): maintains a materialized
/// candidate [`view`](Self::view) and advances it one world [`Delta`] at a
/// time via a footprint index, so per-step cost is `∝ |Δ| × fanout`, never
/// `∝ |world|`.
pub struct IncrementalEngine {
    /// The regimes as owned dataflow operators. Owned (not borrowed) because
    /// [`IncrementalRegime::apply`] takes `&mut self` — a regime may carry
    /// incremental internal state between steps.
    regimes: Vec<Box<dyn IncrementalRegime>>,
    /// The footprint index: configuration handle → the sorted set of regime
    /// indices whose [`Footprint::Configs`] contains it. Built once at
    /// construction; the sole reason per-step routing is sub-linear in the
    /// inert world. `BTreeMap`/`BTreeSet` (Ring0 §0 — never `HashMap`) so
    /// routing order is deterministic.
    index: BTreeMap<Handle, BTreeSet<usize>>,
    /// Regime indices with a [`Footprint::AllConfigs`] footprint — consulted
    /// on *every* non-empty delta (they declared they cannot be skipped).
    universal: BTreeSet<usize>,
    /// The materialized candidate view: the union, over every configuration
    /// currently presented, of every regime's candidates for it.
    view: BTreeSet<Candidate>,
}

impl IncrementalEngine {
    /// Build an engine over `regimes`, computing the footprint index once.
    /// This construction cost may scale with the total declared footprint
    /// size (the world) — that is *setup*, not a committed step, and is not
    /// what ADR-0002 §9.1 bounds.
    pub fn new(regimes: Vec<Box<dyn IncrementalRegime>>) -> Self {
        let mut index: BTreeMap<Handle, BTreeSet<usize>> = BTreeMap::new();
        let mut universal = BTreeSet::new();
        for (i, regime) in regimes.iter().enumerate() {
            match regime.footprint() {
                Footprint::AllConfigs => {
                    universal.insert(i);
                }
                Footprint::Configs(set) => {
                    for h in set {
                        index.entry(h).or_default().insert(i);
                    }
                }
            }
        }
        IncrementalEngine {
            regimes,
            index,
            universal,
            view: BTreeSet::new(),
        }
    }

    /// The materialized candidate view — the incremental counterpart of the
    /// naive oracle's [`crate::oracle::cand`] union over the presented
    /// configuration set. The differential-identity anchor compares this,
    /// byte-for-byte, against [`naive_view_over`].
    pub fn view(&self) -> &BTreeSet<Candidate> {
        &self.view
    }

    /// Number of regimes the engine is driving.
    pub fn regime_count(&self) -> usize {
        self.regimes.len()
    }

    /// Consume one world `delta`: route it — via the footprint index — to
    /// **only** the regimes whose footprint intersects it, merge each
    /// regime's returned [`CandidateDelta`], materialize the combined delta
    /// into the [`view`](Self::view), and return the net delta plus the
    /// per-step [`CostRecord`].
    ///
    /// **Cost accounting (what the O(Δ) gate reads).** One work unit per
    /// touched handle looked up in the index; one per regime actually
    /// applied; one per candidate-delta entry produced. Inert configurations
    /// (never touched by `delta`) and inert regimes (empty footprint, never
    /// in the index) contribute **zero** — which is exactly why doubling
    /// either leaves this cost unchanged (ADR-0002 §9.1).
    pub fn step(&mut self, delta: &Delta) -> StepReport {
        let mut work: u64 = 0;

        // Route: collect the regimes this delta actually reaches. A BTreeSet
        // keeps routing deterministic and de-duplicates a regime reached via
        // several touched handles.
        let mut affected: BTreeSet<usize> = BTreeSet::new();
        if !delta.is_empty() {
            for h in delta.touched() {
                work += 1; // one unit per touched-handle index lookup
                if let Some(regime_indices) = self.index.get(&h) {
                    affected.extend(regime_indices.iter().copied());
                }
            }
            affected.extend(self.universal.iter().copied());
        }

        // Apply only the reached regimes; merge their candidate deltas.
        let mut combined = CandidateDelta::new();
        for &i in &affected {
            work += 1; // one unit per regime actually applied
            let cd = self.regimes[i].apply(delta);
            work += cd.len() as u64; // one unit per candidate-delta entry
            combined.merge(cd);
        }

        // Materialize: removals first, then additions (a candidate that is
        // both removed and re-added this step ends present — the additive
        // half wins, matching a from-scratch recompute).
        for c in &combined.removed {
            self.view.remove(c);
        }
        for c in &combined.added {
            self.view.insert(*c);
        }

        StepReport {
            candidate_delta: combined,
            cost: CostRecord::Steps(work),
        }
    }
}

/// The naive oracle's candidate view recomputed **from scratch** over a whole
/// presented configuration set `configs`: the union, over each config `x`, of
/// [`crate::oracle::cand`] for the exec config `⟨x, policy, history⟩`. This is
/// the reference the incremental [`IncrementalEngine::view`] is checked
/// against (ADR-0002 §9.2 differential-test discipline) and the deliberately
/// `∝ |configs|` baseline the O(Δ) gate's expected-fail case measures.
///
/// `policy`/`history` are held fixed across the set: this models "the same
/// deliberation context, several candidate configurations present at once,"
/// which is exactly what the incremental view accumulates as configs are
/// added and removed.
pub fn naive_view_over(
    regimes: &[&dyn Regime],
    adm: &dyn Adm,
    configs: &BTreeSet<Handle>,
    policy: Handle,
    history: brix_canon::Digest,
) -> BTreeSet<Candidate> {
    let mut out = BTreeSet::new();
    for &world in configs {
        let e = ExecConfig::new(world, policy, history);
        out.extend(crate::oracle::cand(regimes, adm, &e));
    }
    out
}

/// An instrumented [`naive_view_over`] emitting the deterministic work-unit
/// count of the from-scratch recompute (ADR-0001 stage-4a): one unit per
/// `(config, regime)` scan plus one per raw candidate examined. This is the
/// `∝ |world|` cost the naive oracle pays on *every* recompute — the shape
/// the O(Δ) gate's expected-fail case pins, and the incremental engine's flat
/// per-step cost is contrasted against.
pub fn naive_view_over_instrumented(
    regimes: &[&dyn Regime],
    adm: &dyn Adm,
    configs: &BTreeSet<Handle>,
    policy: Handle,
    history: brix_canon::Digest,
) -> (BTreeSet<Candidate>, CostRecord) {
    let mut out = BTreeSet::new();
    let mut work: u64 = 0;
    for &world in configs {
        let e = ExecConfig::new(world, policy, history);
        for regime in regimes {
            work += 1; // one unit per (config, regime) scan — the |world| factor
            for c in regime.candidates(&e) {
                work += 1; // one unit per raw candidate examined
                if adm.admits(&e, &c) {
                    out.insert(c);
                }
            }
        }
    }
    (out, CostRecord::Steps(work))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adm::AdmAll;
    use crate::intern::Interner;
    use brix_canon::{Digest, Domain};

    fn tag(i: &mut Interner, s: &str) -> Handle {
        i.intern(Digest::of(Domain::Value, s.as_bytes()))
    }

    /// A regime sensitive to exactly one config `c`: adding `c` introduces
    /// the reflexive candidate `c → c`; removing `c` withdraws it. Mirrors
    /// the literal-equality regime's incremental shape, scoped to one config
    /// so the engine's routing and materialization can be checked directly.
    /// Implements **both** [`Regime`] (naive) and [`IncrementalRegime`] so a
    /// single fixture drives both sides of the differential-identity check.
    #[derive(Clone, Copy)]
    struct OneConfigRegime {
        regime: Handle,
        config: Handle,
        witness: Handle,
    }

    impl OneConfigRegime {
        fn candidate(&self) -> Candidate {
            Candidate {
                regime: self.regime,
                witness: self.witness,
                successor: self.config,
            }
        }
    }

    impl Regime for OneConfigRegime {
        fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
            if e.world == self.config {
                vec![self.candidate()]
            } else {
                Vec::new()
            }
        }
    }

    impl IncrementalRegime for OneConfigRegime {
        fn footprint(&self) -> Footprint {
            Footprint::configs([self.config])
        }

        fn apply(&mut self, delta: &Delta) -> CandidateDelta {
            let mut cd = CandidateDelta::new();
            if delta.added.contains(&self.config) {
                cd.added.insert(self.candidate());
            }
            if delta.removed.contains(&self.config) {
                cd.removed.insert(self.candidate());
            }
            cd
        }
    }

    #[test]
    fn adding_then_removing_a_config_round_trips_the_view() {
        let mut i = Interner::new();
        let c = tag(&mut i, "c");
        let regime = OneConfigRegime {
            regime: tag(&mut i, "r"),
            config: c,
            witness: tag(&mut i, "w"),
        };
        let expected = regime.candidate();
        let mut engine = IncrementalEngine::new(vec![Box::new(OneConfigRegime {
            regime: regime.regime,
            config: regime.config,
            witness: regime.witness,
        })]);

        let add = engine.step(&Delta::of_added([c]));
        assert_eq!(add.candidate_delta.added, BTreeSet::from([expected]));
        assert_eq!(engine.view(), &BTreeSet::from([expected]));

        let remove = engine.step(&Delta::of_removed([c]));
        assert_eq!(remove.candidate_delta.removed, BTreeSet::from([expected]));
        assert!(engine.view().is_empty());
    }

    #[test]
    fn a_delta_touching_no_footprint_is_a_zero_work_skip() {
        let mut i = Interner::new();
        let c = tag(&mut i, "c");
        let other = tag(&mut i, "other");
        let mut engine = IncrementalEngine::new(vec![Box::new(OneConfigRegime {
            regime: tag(&mut i, "r"),
            config: c,
            witness: tag(&mut i, "w"),
        })]);

        // Delta touches `other`, which no regime's footprint contains: the
        // one touched-handle lookup is paid, but no regime is applied.
        let report = engine.step(&Delta::of_added([other]));
        assert!(report.candidate_delta.is_empty());
        assert_eq!(
            report.cost.work_units(),
            Some(1),
            "only the single index lookup is paid — no regime scanned"
        );
        assert!(engine.view().is_empty());
    }

    #[test]
    fn per_step_cost_is_independent_of_inert_regime_count() {
        // Two engines: one with the active regime alone, one with the active
        // regime plus many inert (empty-footprint) regimes. The same delta
        // must cost the same on both — inert regimes never enter the index.
        struct Inert;
        impl IncrementalRegime for Inert {
            fn footprint(&self) -> Footprint {
                Footprint::empty()
            }
            fn apply(&mut self, _delta: &Delta) -> CandidateDelta {
                CandidateDelta::new()
            }
        }

        let mut i = Interner::new();
        let c = tag(&mut i, "c");
        let rid = tag(&mut i, "r");
        let wid = tag(&mut i, "w");
        let mk_active = || {
            Box::new(OneConfigRegime {
                regime: rid,
                config: c,
                witness: wid,
            }) as Box<dyn IncrementalRegime>
        };

        let mut lean = IncrementalEngine::new(vec![mk_active()]);
        let mut ballasted: Vec<Box<dyn IncrementalRegime>> = vec![mk_active()];
        for _ in 0..1000 {
            ballasted.push(Box::new(Inert));
        }
        let mut ballasted = IncrementalEngine::new(ballasted);

        let d = Delta::of_added([c]);
        let lean_cost = lean.step(&d).cost.work_units().unwrap();
        let ballasted_cost = ballasted.step(&d).cost.work_units().unwrap();
        assert_eq!(
            lean_cost, ballasted_cost,
            "1000 inert regimes must not change per-step cost"
        );
    }

    #[test]
    fn incremental_view_equals_the_naive_recompute_across_a_delta_stream() {
        // Build several one-config regimes; drive the engine through an
        // add/remove stream and, after each step, assert the incremental view
        // equals the naive from-scratch recompute over the present set.
        let mut i = Interner::new();
        let policy = tag(&mut i, "policy");
        let history = Digest::of(Domain::Value, b"h");
        let cs: Vec<Handle> = (0..5).map(|k| tag(&mut i, &format!("c{k}"))).collect();
        let ws: Vec<Handle> = (0..5).map(|k| tag(&mut i, &format!("w{k}"))).collect();
        let rid = tag(&mut i, "r");

        let naive_regimes: Vec<OneConfigRegime> = (0..5)
            .map(|k| OneConfigRegime {
                regime: rid,
                config: cs[k],
                witness: ws[k],
            })
            .collect();
        let engine_regimes: Vec<Box<dyn IncrementalRegime>> = (0..5)
            .map(|k| {
                Box::new(OneConfigRegime {
                    regime: rid,
                    config: cs[k],
                    witness: ws[k],
                }) as Box<dyn IncrementalRegime>
            })
            .collect();
        let mut engine = IncrementalEngine::new(engine_regimes);

        let mut present: BTreeSet<Handle> = BTreeSet::new();
        let stream = [
            Delta::of_added([cs[0], cs[2]]),
            Delta::of_added([cs[4]]),
            Delta::of_removed([cs[2]]),
            Delta::of_added([cs[1], cs[3]]),
            Delta::of_removed([cs[0], cs[4]]),
        ];
        let naive_view: Vec<&dyn Regime> = naive_regimes.iter().map(|r| r as &dyn Regime).collect();
        for d in stream {
            for h in &d.added {
                present.insert(*h);
            }
            for h in &d.removed {
                present.remove(h);
            }
            engine.step(&d);
            let expected = naive_view_over(&naive_view, &AdmAll, &present, policy, history);
            assert_eq!(
                engine.view(),
                &expected,
                "incremental view must equal the naive recompute after every step"
            );
        }
    }
}
