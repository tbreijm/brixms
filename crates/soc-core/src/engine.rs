//! The incremental engine (ADR-0002 §9.1 "THE invariant", §9.2 "Candidates
//! are a materialized incremental view"; `Build_Plan_v3_SOC.md` Step 6, E3):
//! the delta-driven counterpart to the naive [`crate::oracle`], landing
//! **beside** it — never replacing it (the naive oracle is the retained
//! reference oracle the fast engine is differential-tested against, ADR-0002
//! §9.2).
//!
//! A provider, as a *running engine component*, is a **dataflow operator**
//! ([`IncrementalWitnessIndex`]): it declares a [`Footprint`] and, given a world
//! [`Delta`], returns only the [`CandidateDelta`] it induces — never a re-run
//! of the whole `ρ_w` relation.
//!
//! [`IncrementalEngine`] maintains the materialized candidate **view** as a
//! `BTreeSet<Candidate>` and keeps it in step with a stream of world deltas.
//! The one mechanism that earns the O(Δ) invariant is the **footprint
//! index**: built once at construction from every provider's declared
//! footprint, it lets [`IncrementalEngine::step`] route a delta to *only* the
//! providers whose footprint intersects it, in `O(|Δ| × fanout)`, never
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
use crate::witness_provider::{Candidate, WitnessProvider};

/// A realization provider presented as an incremental **dataflow operator**
/// (ADR-0002 §9.2). The delta-driven counterpart to [`WitnessProvider::candidates`]:
/// where the naive [`WitnessProvider`] recomputes its *entire* candidate set from an
/// [`ExecConfig`] on every call, an `IncrementalWitnessIndex` declares which
/// configurations it is sensitive to ([`footprint`](Self::footprint)) and,
/// given a world [`Delta`], returns only the [`CandidateDelta`] that delta
/// induces ([`apply`](Self::apply)).
///
/// The two traits are deliberately independent (`soc-core` never modifies the
/// naive `WitnessProvider`): a provider type may implement both, and the differential
/// gate relies on it — the incremental `apply` stream must reconstruct
/// exactly the naive `candidates` union (see [`naive_view_over`]).
pub trait IncrementalWitnessIndex {
    /// The set of configurations this provider is sensitive to. The engine
    /// **skips** this provider entirely for any delta whose footprint does not
    /// intersect — that skip is the O(Δ) invariant (ADR-0002 §9.1). A
    /// footprint must be honest: it MUST include every configuration for
    /// which [`apply`](Self::apply) could produce a non-empty candidate
    /// delta, or the engine will silently drop candidates the naive oracle
    /// keeps (a differential-identity failure).
    fn footprint(&self) -> Footprint;

    /// Consume one world `delta` and return **only** the candidate delta it
    /// induces for this provider. Called by the engine solely when the
    /// provider's [`footprint`](Self::footprint) intersects `delta` — an
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
    /// applied providers plus produced candidate-delta entries. Provably
    /// independent of the inert configuration/provider population (see the
    /// module docs and `tests/o_delta_gate.rs`).
    pub cost: CostRecord,
}

/// The incremental engine (ADR-0002 §9.1/§9.2): maintains a materialized
/// candidate [`view`](Self::view) and advances it one world [`Delta`] at a
/// time via a footprint index, so per-step cost is `∝ |Δ| × fanout`, never
/// `∝ |world|`.
pub struct IncrementalEngine {
    /// The providers as owned dataflow operators. Owned (not borrowed) because
    /// [`IncrementalWitnessIndex::apply`] takes `&mut self` — a provider may carry
    /// incremental internal state between steps.
    providers: Vec<Box<dyn IncrementalWitnessIndex>>,
    /// The footprint index: configuration handle → the sorted set of provider
    /// indices whose [`Footprint::Configs`] contains it. Built once at
    /// construction; the sole reason per-step routing is sub-linear in the
    /// inert world. `BTreeMap`/`BTreeSet` (Ring0 §0 — never `HashMap`) so
    /// routing order is deterministic.
    index: BTreeMap<Handle, BTreeSet<usize>>,
    /// WitnessProvider indices with a [`Footprint::AllConfigs`] footprint — consulted
    /// on *every* non-empty delta (they declared they cannot be skipped).
    universal: BTreeSet<usize>,
    /// The materialized candidate view: the union, over every configuration
    /// currently presented, of every provider's candidates for it.
    view: BTreeSet<Candidate>,
}

impl IncrementalEngine {
    /// Build an engine over `providers`, computing the footprint index once.
    /// This construction cost may scale with the total declared footprint
    /// size (the world) — that is *setup*, not a committed step, and is not
    /// what ADR-0002 §9.1 bounds.
    pub fn new(providers: Vec<Box<dyn IncrementalWitnessIndex>>) -> Self {
        let mut index: BTreeMap<Handle, BTreeSet<usize>> = BTreeMap::new();
        let mut universal = BTreeSet::new();
        for (i, provider) in providers.iter().enumerate() {
            match provider.footprint() {
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
            providers,
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

    /// Number of providers the engine is driving.
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    /// Consume one world `delta`: route it — via the footprint index — to
    /// **only** the providers whose footprint intersects it, merge each
    /// provider's returned [`CandidateDelta`], materialize the combined delta
    /// into the [`view`](Self::view), and return the net delta plus the
    /// per-step [`CostRecord`].
    ///
    /// **Cost accounting (what the O(Δ) gate reads).** One work unit per
    /// touched handle looked up in the index; one per provider actually
    /// applied; one per candidate-delta entry produced. Inert configurations
    /// (never touched by `delta`) and inert providers (empty footprint, never
    /// in the index) contribute **zero** — which is exactly why doubling
    /// either leaves this cost unchanged (ADR-0002 §9.1).
    pub fn step(&mut self, delta: &Delta) -> StepReport {
        let mut work: u64 = 0;

        // Route: collect the providers this delta actually reaches. A BTreeSet
        // keeps routing deterministic and de-duplicates a provider reached via
        // several touched handles.
        let mut affected: BTreeSet<usize> = BTreeSet::new();
        if !delta.is_empty() {
            for h in delta.touched() {
                work += 1; // one unit per touched-handle index lookup
                if let Some(provider_indices) = self.index.get(&h) {
                    affected.extend(provider_indices.iter().copied());
                }
            }
            affected.extend(self.universal.iter().copied());
        }

        // Apply only the reached providers; merge their candidate deltas.
        let mut combined = CandidateDelta::new();
        for &i in &affected {
            work += 1; // one unit per provider actually applied
            let cd = self.providers[i].apply(delta);
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
    providers: &[&dyn WitnessProvider],
    adm: &dyn Adm,
    configs: &BTreeSet<Handle>,
    policy: Handle,
    history: brix_canon::Digest,
) -> BTreeSet<Candidate> {
    let mut out = BTreeSet::new();
    for &world in configs {
        let e = ExecConfig::new(world, policy, history);
        out.extend(crate::oracle::cand(providers, adm, &e));
    }
    out
}

/// An instrumented [`naive_view_over`] emitting the deterministic work-unit
/// count of the from-scratch recompute (ADR-0001 stage-4a): one unit per
/// `(config, provider)` scan plus one per raw candidate examined. This is the
/// `∝ |world|` cost the naive oracle pays on *every* recompute — the shape
/// the O(Δ) gate's expected-fail case pins, and the incremental engine's flat
/// per-step cost is contrasted against.
pub fn naive_view_over_instrumented(
    providers: &[&dyn WitnessProvider],
    adm: &dyn Adm,
    configs: &BTreeSet<Handle>,
    policy: Handle,
    history: brix_canon::Digest,
) -> (BTreeSet<Candidate>, CostRecord) {
    let mut out = BTreeSet::new();
    let mut work: u64 = 0;
    for &world in configs {
        let e = ExecConfig::new(world, policy, history);
        for provider in providers {
            work += 1; // one unit per (config, provider) scan — the |world| factor
            for c in provider.candidates(&e) {
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

    /// A provider sensitive to exactly one config `c`: adding `c` introduces
    /// the reflexive candidate `c → c`; removing `c` withdraws it. Mirrors
    /// the literal-equality provider's incremental shape, scoped to one config
    /// so the engine's routing and materialization can be checked directly.
    /// Implements **both** [`WitnessProvider`] (naive) and [`IncrementalWitnessIndex`] so a
    /// single fixture drives both sides of the differential-identity check.
    #[derive(Clone, Copy)]
    struct OneConfigProvider {
        config: Handle,
        witness: Handle,
    }

    impl OneConfigProvider {
        fn candidate(&self) -> Candidate {
            Candidate {
                witness: self.witness,
                successor: self.config,
            }
        }
    }

    impl WitnessProvider for OneConfigProvider {
        fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
            if e.world == self.config {
                vec![self.candidate()]
            } else {
                Vec::new()
            }
        }
    }

    impl IncrementalWitnessIndex for OneConfigProvider {
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
        let provider = OneConfigProvider {
            config: c,
            witness: tag(&mut i, "w"),
        };
        let expected = provider.candidate();
        let mut engine = IncrementalEngine::new(vec![Box::new(OneConfigProvider {
            config: provider.config,
            witness: provider.witness,
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
        let mut engine = IncrementalEngine::new(vec![Box::new(OneConfigProvider {
            config: c,
            witness: tag(&mut i, "w"),
        })]);

        // Delta touches `other`, which no provider's footprint contains: the
        // one touched-handle lookup is paid, but no provider is applied.
        let report = engine.step(&Delta::of_added([other]));
        assert!(report.candidate_delta.is_empty());
        assert_eq!(
            report.cost.work_units(),
            Some(1),
            "only the single index lookup is paid — no provider scanned"
        );
        assert!(engine.view().is_empty());
    }

    #[test]
    fn per_step_cost_is_independent_of_inert_provider_count() {
        // Two engines: one with the active provider alone, one with the active
        // provider plus many inert (empty-footprint) providers. The same delta
        // must cost the same on both — inert providers never enter the index.
        struct Inert;
        impl IncrementalWitnessIndex for Inert {
            fn footprint(&self) -> Footprint {
                Footprint::empty()
            }
            fn apply(&mut self, _delta: &Delta) -> CandidateDelta {
                CandidateDelta::new()
            }
        }

        let mut i = Interner::new();
        let c = tag(&mut i, "c");
        let wid = tag(&mut i, "w");
        let mk_active = || {
            Box::new(OneConfigProvider {
                config: c,
                witness: wid,
            }) as Box<dyn IncrementalWitnessIndex>
        };

        let mut lean = IncrementalEngine::new(vec![mk_active()]);
        let mut ballasted: Vec<Box<dyn IncrementalWitnessIndex>> = vec![mk_active()];
        for _ in 0..1000 {
            ballasted.push(Box::new(Inert));
        }
        let mut ballasted = IncrementalEngine::new(ballasted);

        let d = Delta::of_added([c]);
        let lean_cost = lean.step(&d).cost.work_units().unwrap();
        let ballasted_cost = ballasted.step(&d).cost.work_units().unwrap();
        assert_eq!(
            lean_cost, ballasted_cost,
            "1000 inert providers must not change per-step cost"
        );
    }

    #[test]
    fn incremental_view_equals_the_naive_recompute_across_a_delta_stream() {
        // Build several one-config providers; drive the engine through an
        // add/remove stream and, after each step, assert the incremental view
        // equals the naive from-scratch recompute over the present set.
        let mut i = Interner::new();
        let policy = tag(&mut i, "policy");
        let history = Digest::of(Domain::Value, b"h");
        let cs: Vec<Handle> = (0..5).map(|k| tag(&mut i, &format!("c{k}"))).collect();
        let ws: Vec<Handle> = (0..5).map(|k| tag(&mut i, &format!("w{k}"))).collect();

        let naive_providers: Vec<OneConfigProvider> = (0..5)
            .map(|k| OneConfigProvider {
                config: cs[k],
                witness: ws[k],
            })
            .collect();
        let engine_providers: Vec<Box<dyn IncrementalWitnessIndex>> = (0..5)
            .map(|k| {
                Box::new(OneConfigProvider {
                    config: cs[k],
                    witness: ws[k],
                }) as Box<dyn IncrementalWitnessIndex>
            })
            .collect();
        let mut engine = IncrementalEngine::new(engine_providers);

        let mut present: BTreeSet<Handle> = BTreeSet::new();
        let stream = [
            Delta::of_added([cs[0], cs[2]]),
            Delta::of_added([cs[4]]),
            Delta::of_removed([cs[2]]),
            Delta::of_added([cs[1], cs[3]]),
            Delta::of_removed([cs[0], cs[4]]),
        ];
        let naive_view: Vec<&dyn WitnessProvider> = naive_providers
            .iter()
            .map(|r| r as &dyn WitnessProvider)
            .collect();
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
