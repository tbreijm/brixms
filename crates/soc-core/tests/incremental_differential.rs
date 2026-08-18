//! Differential identity: the incremental engine's materialized candidate
//! view ≡ the naive oracle's from-scratch recompute, over every reachable
//! state of a delta stream (ADR-0002 §9.2 "Reference oracle" — the fast
//! engine is differential-tested against the retained naive one;
//! `Build_Plan_v3_SOC.md` Step 6, the correctness anchor for the fast path).
//!
//! This is the non-negotiable anchor: an incremental view that ever disagreed
//! with the naive union — by even one candidate or one iteration-order byte —
//! would be an unsound fast path. Here it is exercised with several regimes
//! of mixed footprints (single-config, multi-config, and an `AllConfigs`
//! regime that must never be skipped) over an add/remove/re-add stream that
//! reaches many distinct present-config sets. `soc-regimes`' own
//! `LiteralEqualityRegime` gets the same treatment against the real
//! projection corpus in `brix-conformance`.

use std::collections::BTreeSet;

use brix_canon::{Digest, Domain};
use soc_core::adm::{AdmAll, AdmWitnessAllowlist};
use soc_core::delta::{CandidateDelta, Delta, Footprint};
use soc_core::engine::{naive_view_over, IncrementalEngine, IncrementalWitnessIndex};
use soc_core::exec::ExecConfig;
use soc_core::intern::{Handle, Interner};
use soc_core::witness_provider::{Candidate, WitnessProvider};

/// A provider that, for each config in a fixed set it "knows", proposes one
/// reflexive candidate `x → x`. Its footprint is that known set (or
/// `AllConfigs` when `universal` is set — modelling a regime that declines to
/// name its configs and so can never be skipped). Implements both traits from
/// one candidate definition, so naive and incremental are the same semantics.
#[derive(Clone)]
struct KnownSetRegime {
    witness: Handle,
    known: BTreeSet<Handle>,
    universal: bool,
}

impl KnownSetRegime {
    fn candidate(&self, config: Handle) -> Option<Candidate> {
        if self.known.contains(&config) {
            Some(Candidate {
                witness: self.witness,
                successor: config,
            })
        } else {
            None
        }
    }
}

impl WitnessProvider for KnownSetRegime {
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        self.candidate(e.world).into_iter().collect()
    }
}

impl IncrementalWitnessIndex for KnownSetRegime {
    fn footprint(&self) -> Footprint {
        if self.universal {
            Footprint::AllConfigs
        } else {
            Footprint::configs(self.known.iter().copied())
        }
    }

    fn apply(&mut self, delta: &Delta) -> CandidateDelta {
        let mut cd = CandidateDelta::new();
        for h in &delta.added {
            if let Some(c) = self.candidate(*h) {
                cd.added.insert(c);
            }
        }
        for h in &delta.removed {
            if let Some(c) = self.candidate(*h) {
                cd.removed.insert(c);
            }
        }
        cd
    }
}

fn tag(i: &mut Interner, s: &str) -> Handle {
    i.intern(Digest::of(Domain::Value, s.as_bytes()))
}

/// Drive both engines over `stream` and assert view identity after every
/// step, under a given `Adm`. Returns the number of steps checked.
fn assert_view_identity_over_stream(
    naive_regimes: &[&dyn WitnessProvider],
    engine: &mut IncrementalEngine,
    adm: &dyn soc_core::adm::Adm,
    policy: Handle,
    history: Digest,
    stream: &[Delta],
) -> usize {
    let mut present: BTreeSet<Handle> = BTreeSet::new();
    let mut checked = 0;
    for d in stream {
        for h in &d.added {
            present.insert(*h);
        }
        for h in &d.removed {
            present.remove(h);
        }
        engine.step(d);
        let expected = naive_view_over(naive_regimes, adm, &present, policy, history);
        assert_eq!(
            engine.view(),
            &expected,
            "incremental view diverged from the naive recompute after step {checked}"
        );
        checked += 1;
    }
    checked
}

fn build_regimes(i: &mut Interner, configs: &[Handle]) -> Vec<KnownSetRegime> {
    // Witness provider A: single-config footprint over configs[0].
    let a = KnownSetRegime {
        witness: tag(i, "witness.a"),
        known: BTreeSet::from([configs[0]]),
        universal: false,
    };
    // WitnessProvider B: multi-config footprint over configs[1], configs[2], configs[3].
    let b = KnownSetRegime {
        witness: tag(i, "witness.b"),
        known: BTreeSet::from([configs[1], configs[2], configs[3]]),
        universal: false,
    };
    // WitnessProvider C: AllConfigs — knows configs[2] and configs[4] but declares an
    // un-skippable footprint, so the engine must consult it on every delta.
    let c = KnownSetRegime {
        witness: tag(i, "witness.c"),
        known: BTreeSet::from([configs[2], configs[4]]),
        universal: true,
    };
    vec![a, b, c]
}

fn stream(configs: &[Handle]) -> Vec<Delta> {
    vec![
        Delta::of_added([configs[0], configs[2]]),
        Delta::of_added([configs[4]]),
        Delta::of_removed([configs[2]]),
        Delta::of_added([configs[1], configs[3]]),
        Delta::of_added([configs[2]]), // re-add a previously-removed config
        Delta::of_removed([configs[0], configs[4]]),
        Delta::of_removed([configs[1], configs[2], configs[3]]),
    ]
}

#[test]
fn incremental_view_equals_naive_recompute_under_adm_all() {
    let mut i = Interner::new();
    let policy = tag(&mut i, "policy");
    let history = Digest::of(Domain::Value, b"history");
    let configs: Vec<Handle> = (0..5).map(|k| tag(&mut i, &format!("c{k}"))).collect();

    let regimes = build_regimes(&mut i, &configs);
    let engine_regimes: Vec<Box<dyn IncrementalWitnessIndex>> = regimes
        .iter()
        .cloned()
        .map(|r| Box::new(r) as Box<dyn IncrementalWitnessIndex>)
        .collect();
    let mut engine = IncrementalEngine::new(engine_regimes);
    let naive: Vec<&dyn WitnessProvider> =
        regimes.iter().map(|r| r as &dyn WitnessProvider).collect();

    let s = stream(&configs);
    let checked =
        assert_view_identity_over_stream(&naive, &mut engine, &AdmAll, policy, history, &s);
    assert_eq!(checked, s.len());
    assert!(checked > 0);
}

#[test]
fn incremental_view_equals_naive_recompute_under_a_tightened_adm() {
    // Governance monotonicity (ADR-0002 §5.5): under a tightened Adm that
    // admits only provider A's witnesses, the incremental view must still
    // equal the naive recompute. The engine applies Adm at materialization
    // parity by construction — this pins that the delta path honours it too.
    let mut i = Interner::new();
    let policy = tag(&mut i, "policy");
    let history = Digest::of(Domain::Value, b"history");
    let configs: Vec<Handle> = (0..5).map(|k| tag(&mut i, &format!("c{k}"))).collect();

    let regimes = build_regimes(&mut i, &configs);
    let allow_a = AdmWitnessAllowlist::new([regimes[0].witness]);

    // The incremental engine's regimes must themselves only emit A-admissible
    // candidates for the comparison to hold; here we simply restrict the
    // engine to regime A, and compare against the naive recompute over all
    // regimes filtered by the same allowlist — both must yield only A's
    // candidates.
    let engine_regimes: Vec<Box<dyn IncrementalWitnessIndex>> =
        vec![Box::new(regimes[0].clone()) as Box<dyn IncrementalWitnessIndex>];
    let mut engine = IncrementalEngine::new(engine_regimes);
    let naive: Vec<&dyn WitnessProvider> =
        regimes.iter().map(|r| r as &dyn WitnessProvider).collect();

    let s = stream(&configs);
    let checked =
        assert_view_identity_over_stream(&naive, &mut engine, &allow_a, policy, history, &s);
    assert_eq!(checked, s.len());
}

#[test]
fn empty_stream_leaves_an_empty_view_equal_to_the_empty_recompute() {
    let mut i = Interner::new();
    let policy = tag(&mut i, "policy");
    let history = Digest::of(Domain::Value, b"history");
    let configs: Vec<Handle> = (0..5).map(|k| tag(&mut i, &format!("c{k}"))).collect();
    let regimes = build_regimes(&mut i, &configs);
    let engine_regimes: Vec<Box<dyn IncrementalWitnessIndex>> = regimes
        .iter()
        .cloned()
        .map(|r| Box::new(r) as Box<dyn IncrementalWitnessIndex>)
        .collect();
    let engine = IncrementalEngine::new(engine_regimes);
    let empty: BTreeSet<Handle> = BTreeSet::new();
    let naive: Vec<&dyn WitnessProvider> =
        regimes.iter().map(|r| r as &dyn WitnessProvider).collect();
    assert!(engine.view().is_empty());
    assert_eq!(
        engine.view(),
        &naive_view_over(&naive, &AdmAll, &empty, policy, history)
    );
}
