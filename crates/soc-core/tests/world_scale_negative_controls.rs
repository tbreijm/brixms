//! Negative scale controls for the existing full-state paths (ADR-0046, P0).
//!
//! Baseline verification that `ArcMap::insert` and `Frontier::apply_delta`
//! cost grows with the size of the store / frontier they are applied to, so
//! the O(Δ) gate rejects them, while the SOC routing gate stays flat.
//!
//! **What is measured.** The stored value type [`Counted`] increments a
//! per-thread counter every time it is cloned. Cloning a `BTreeMap` clones
//! every value it holds, so the number of `Counted` clones during an operation
//! is a count of entries the code *actually copied*. It is observed from the
//! operation, never computed from the container's length. (No custom allocator:
//! the workspace denies `unsafe_code`, and this stays inside that policy.)
//!
//! Limits, stated plainly: this counts value clones only. Keys cloned, bytes
//! allocated, nodes visited/hashed and tuple probes are **not** measured here
//! and are not reported as zero; those structural counters need the P1
//! structures and arrive with them. Wall-clock evidence lives in the benches.
//!
//! Governed by `docs/planning/persistent-world-runtime-plan.md` §1, §4 (P0).
//! Run with `--nocapture` to see the raw measurements.

use std::cell::Cell;

use brix_canon::{Digest, Domain};
use soc_core::calendar::{Frontier, Key};
use soc_core::delta::{CandidateDelta, Delta, Footprint};
use soc_core::engine::{IncrementalEngine, IncrementalWitnessIndex};
use soc_core::intern::{Handle, Interner};
use soc_core::store::{ArcMap, PersistentMap};
use soc_core::witness_provider::Candidate;

// ---------------------------------------------------------------------------
// Clone-counting value
// ---------------------------------------------------------------------------

thread_local! {
    static CLONES: Cell<u64> = const { Cell::new(0) };
}

/// A value that records each `clone()` on the current thread. Per-thread so
/// parallel tests in this binary cannot pollute each other's counts.
#[derive(Debug, PartialEq, Eq)]
struct Counted(u64);

impl Clone for Counted {
    fn clone(&self) -> Self {
        CLONES.with(|c| c.set(c.get() + 1));
        Counted(self.0)
    }
}

/// Run `f` and return how many `Counted` clones it performed.
fn count_clones<T>(f: impl FnOnce() -> T) -> (T, u64) {
    let before = CLONES.with(Cell::get);
    let out = f();
    (out, CLONES.with(Cell::get) - before)
}

fn make_digest(tag: &str, n: u64) -> Digest {
    Digest::of(Domain::Value, format!("{tag}_{n}").as_bytes())
}

/// Doubling the input must grow measured cost by at least this factor for the
/// control to count as "world-proportional". A flat path would be ~1.0.
const MIN_DOUBLING_GROWTH: f64 = 1.5;

fn assert_world_proportional(what: &str, series: &[(usize, u64)]) {
    for w in series.windows(2) {
        let (n0, c0) = w[0];
        let (n1, c1) = w[1];
        let growth = c1 as f64 / c0 as f64;
        assert!(
            growth >= MIN_DOUBLING_GROWTH,
            "{what}: expected world-proportional cost but N {n0}->{n1} grew clones only \
             {growth:.2}x ({c0} -> {c1}); the negative control no longer discriminates"
        );
    }
}

// ---------------------------------------------------------------------------
// 1. Full-store control: ArcMap::insert
// ---------------------------------------------------------------------------

fn measure_arc_map_insert(size: usize) -> u64 {
    let mut map: ArcMap<u64, Counted> = ArcMap::new();
    for i in 0..size as u64 {
        map = map.insert(i, Counted(i * 10));
    }
    assert_eq!(map.len(), size);
    let (next, clones) = count_clones(|| map.insert(size as u64 + 100_000, Counted(999)));
    assert_eq!(next.len(), size + 1);
    clones
}

#[test]
fn negative_control_arc_map_insert_copies_grow_with_store() {
    let series: Vec<(usize, u64)> = [1_000usize, 2_000, 4_000]
        .iter()
        .map(|&n| (n, measure_arc_map_insert(n)))
        .collect();
    for (n, c) in &series {
        eprintln!("ArcMap::insert (1 key)        N={n:>6}: {c} entry clones");
    }
    assert_world_proportional("ArcMap::insert", &series);
}

// ---------------------------------------------------------------------------
// 2. Full-frontier control: Frontier::apply_delta
// ---------------------------------------------------------------------------

fn measure_frontier_delta(size: usize) -> u64 {
    let mut frontier: Frontier<Counted> = Frontier::new();
    for i in 0..size as u64 {
        frontier
            .insert(Key::new(1, 100, make_digest("cand", i)), Counted(i))
            .unwrap();
    }
    let removals = [(Key::new(1, 100, make_digest("cand", 0)), Counted(0))];
    let additions = [(
        Key::new(1, 100, make_digest("new_cand", 9999)),
        Counted(777),
    )];
    let (res, clones) = count_clones(|| frontier.naive_apply_delta(&removals, &additions));
    res.unwrap();
    assert_eq!(frontier.len(), size);
    clones
}

#[test]
fn negative_control_frontier_apply_delta_copies_grow_with_frontier() {
    let series: Vec<(usize, u64)> = [500usize, 1_000, 2_000]
        .iter()
        .map(|&n| (n, measure_frontier_delta(n)))
        .collect();
    for (n, c) in &series {
        eprintln!("Frontier::naive_apply_delta (1 rm+1 add) N={n:>6}: {c} entry clones");
    }
    assert_world_proportional("Frontier::naive_apply_delta", &series);
}

// ---------------------------------------------------------------------------
// 3. The harness must be able to tell flat from proportional
// ---------------------------------------------------------------------------

/// A genuinely constant-cost read on the same container types must register
/// as constant under the same counter. If this failed, the two controls above
/// would prove nothing about the harness's ability to discriminate.
#[test]
fn harness_reads_a_constant_cost_operation_as_flat() {
    let read_one = |n: usize| {
        let mut map: ArcMap<u64, Counted> = ArcMap::new();
        for i in 0..n as u64 {
            map = map.insert(i, Counted(i));
        }
        let (got, clones) = count_clones(|| map.get(&(n as u64 / 2)).cloned());
        assert!(got.is_some());
        clones
    };
    let a = read_one(1_000);
    let b = read_one(4_000);
    assert_eq!(a, 1, "one targeted read clones exactly one value");
    assert_eq!(a, b, "constant-cost read must cost the same at any N");
}

// ---------------------------------------------------------------------------
// 4. The SOC routing gate stays flat (unchanged invariant)
// ---------------------------------------------------------------------------

struct TestRegime {
    active: Handle,
    witness: Handle,
}

impl IncrementalWitnessIndex for TestRegime {
    fn footprint(&self) -> Footprint {
        Footprint::configs([self.active])
    }

    fn apply(&mut self, delta: &Delta) -> CandidateDelta {
        let mut cd = CandidateDelta::new();
        let cand = Candidate {
            witness: self.witness,
            successor: self.active,
        };
        if delta.added.contains(&self.active) {
            cd.added.insert(cand);
        }
        if delta.removed.contains(&self.active) {
            cd.removed.insert(cand);
        }
        cd
    }
}

#[test]
fn routing_gate_incremental_engine_remains_flat() {
    let mut interner = Interner::new();
    let active = interner.intern(make_digest("active", 1));
    let witness = interner.intern(make_digest("witness", 1));

    let run_with_inert = |n_inert: usize| -> u64 {
        let mut i = interner.clone();
        for k in 0..n_inert as u64 {
            i.intern(make_digest("inert", k));
        }
        let mut engine = IncrementalEngine::new(vec![Box::new(TestRegime { active, witness })]);
        let mut delta = Delta::new();
        delta.added.insert(active);
        engine
            .step(&delta)
            .cost
            .work_units()
            .expect("work units must be measured")
    };

    let costs: Vec<u64> = [1_000usize, 2_000, 4_000]
        .iter()
        .map(|&n| run_with_inert(n))
        .collect();
    eprintln!("routing gate incremental work units at 1k/2k/4k inert: {costs:?}");
    assert!(costs.windows(2).all(|w| w[0] == w[1]), "{costs:?}");
}
