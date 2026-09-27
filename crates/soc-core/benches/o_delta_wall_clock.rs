//! Wall-clock companion to the deterministic O(Δ) gate
//! (`tests/o_delta_gate.rs`).
//!
//! That gate measures the incremental engine's self-reported work-unit count
//! (`CostRecord::Steps`) and shows it is flat in the inert-configuration
//! population, while the naive from-scratch oracle's grows with it. Those
//! counts are deterministic and CI-safe, but they are not wall-clock time —
//! a "flat" step could still, in principle, hide a constant-factor cost that
//! only shows up on the clock (allocation, cache behavior, `BTreeMap` node
//! sizes). This benchmark asks the same question with `std::time::Instant`
//! instead of the cost model, over the same fixture shape, at 1k / 10k / 100k
//! inert configurations.
//!
//! `harness = false`, `std` only: no `criterion` (not Ring-0 whitelisted).
//! Warms up, then times a fixed number of iterations and reports the median
//! and p90. Run with `cargo bench -p soc-core --bench o_delta_wall_clock`.
//!
//! This binary is a `[[bench]]` target, so `cargo test` never builds or runs
//! it — only `cargo bench` does.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use brix_canon::{Digest, Domain};
use soc_core::adm::AdmAll;
use soc_core::delta::{CandidateDelta, Delta, Footprint};
use soc_core::engine::{naive_view_over_instrumented, IncrementalEngine, IncrementalWitnessIndex};
use soc_core::exec::ExecConfig;
use soc_core::intern::{Handle, Interner};
use soc_core::witness_provider::{Candidate, WitnessProvider};

/// Mirrors `tests/o_delta_gate.rs`'s `ActiveRegime` fixture exactly (see that
/// file's module docs for why inert *configurations*, not inert regimes, are
/// the right stand-in for ADR-0002 §9.1's own wording). Duplicated here
/// rather than shared because bench targets are separate compilation units
/// from the integration-test binary, and this fixture is ~15 lines.
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

/// Sorted-durations -> (median, p90). `durations` must be non-empty.
fn stats(mut durations: Vec<Duration>) -> (Duration, Duration) {
    durations.sort();
    let median = durations[durations.len() / 2];
    let p90_idx = ((durations.len() * 90) / 100).min(durations.len() - 1);
    (median, durations[p90_idx])
}

fn fmt_ns(d: Duration) -> String {
    let ns = d.as_nanos();
    if ns >= 1_000_000 {
        format!("{:.3} ms", d.as_secs_f64() * 1e3)
    } else if ns >= 1_000 {
        format!("{:.3} us", ns as f64 / 1e3)
    } else {
        format!("{ns} ns")
    }
}

const WARMUP: usize = 10;
const ITERS_ENGINE: usize = 500;
const ITERS_NAIVE_SMALL: usize = 200;
const ITERS_NAIVE_LARGE: usize = 30;
const SCALES: [usize; 3] = [1_000, 10_000, 100_000];

/// Time the incremental engine's per-step wall cost, after `n_inert` inert
/// configurations are already present (ingested unmeasured, as setup). The
/// same active/inactive pair is toggled repeatedly so the fixture is built
/// exactly once per scale rather than once per iteration.
fn bench_incremental_step(n_inert: usize) -> (Duration, Vec<Duration>, Duration) {
    let mut interner = Interner::new();
    let active = tag(&mut interner, "bench.active");
    let witness = tag(&mut interner, "bench.witness");
    let regime = ActiveRegime { active, witness };
    let mut engine = IncrementalEngine::new(vec![Box::new(regime)]);

    let ingest_start = Instant::now();
    for k in 0..n_inert {
        let h = tag(&mut interner, &format!("bench.inert.{k}"));
        let report = engine.step(&Delta::of_added([h]));
        assert!(report.candidate_delta.is_empty());
    }
    let ingest_elapsed = ingest_start.elapsed();

    // Warmup: not timed, but exercises the exact add/remove toggle so the
    // first timed iteration is not paying one-time branch-predictor/cache
    // warmup cost that every later iteration would not.
    for _ in 0..WARMUP {
        engine.step(&Delta::of_added([active]));
        engine.step(&Delta::of_removed([active]));
    }

    let mut add_durations = Vec::with_capacity(ITERS_ENGINE);
    for _ in 0..ITERS_ENGINE {
        let start = Instant::now();
        let report = engine.step(&Delta::of_added([active]));
        add_durations.push(start.elapsed());
        assert_eq!(report.candidate_delta.added.len(), 1);
        engine.step(&Delta::of_removed([active]));
    }

    let (median, _p90) = stats(add_durations.clone());
    (median, add_durations, ingest_elapsed)
}

/// Time the naive from-scratch oracle's per-call wall cost over `n_inert + 1`
/// present configurations.
fn bench_naive(n_inert: usize, iters: usize) -> (Duration, Duration) {
    let mut interner = Interner::new();
    let active = tag(&mut interner, "bench.naive.active");
    let witness = tag(&mut interner, "bench.naive.witness");
    let policy = tag(&mut interner, "bench.naive.policy");
    let history = Digest::of(Domain::Value, b"bench.naive.h0");
    let regime = ActiveRegime { active, witness };
    let regimes: Vec<&dyn WitnessProvider> = vec![&regime];

    let mut present: BTreeSet<Handle> = (0..n_inert)
        .map(|k| tag(&mut interner, &format!("bench.naive.inert.{k}")))
        .collect();
    present.insert(active);

    for _ in 0..WARMUP.min(iters) {
        let _ = naive_view_over_instrumented(&regimes, &AdmAll, &present, policy, history);
    }

    let mut durations = Vec::with_capacity(iters);
    for _ in 0..iters {
        let start = Instant::now();
        let (_view, _cost) =
            naive_view_over_instrumented(&regimes, &AdmAll, &present, policy, history);
        durations.push(start.elapsed());
    }
    stats(durations)
}

fn main() {
    println!("# O(Δ) wall-clock companion (soc-core)\n");
    println!("Machine: this process's host (see docs/performance.md for the CI/container spec).");
    println!(
        "Method: warmup {WARMUP} iterations discarded, then median/p90 over N timed iterations.\n"
    );

    println!(
        "## IncrementalEngine::step — one committed step (add) after N inert configs are present\n"
    );
    println!(
        "{:>10} | {:>9} | {:>12} | {:>12} | {:>14}",
        "N_inert", "iters", "median", "p90", "bulk ingest(N)"
    );
    println!("{}", "-".repeat(70));
    let mut engine_medians = Vec::new();
    for &n in &SCALES {
        let (median, add_durations, ingest) = bench_incremental_step(n);
        let (_median2, p90) = stats(add_durations);
        engine_medians.push((n, median));
        println!(
            "{:>10} | {:>9} | {:>12} | {:>12} | {:>14}",
            n,
            ITERS_ENGINE,
            fmt_ns(median),
            fmt_ns(p90),
            fmt_ns(ingest)
        );
    }

    println!("\n## naive_view_over_instrumented — full recompute over N+1 present configs\n");
    println!(
        "{:>10} | {:>9} | {:>12} | {:>12}",
        "N_inert", "iters", "median", "p90"
    );
    println!("{}", "-".repeat(56));
    let mut naive_medians = Vec::new();
    for &n in &SCALES {
        let iters = if n >= 100_000 {
            ITERS_NAIVE_LARGE
        } else {
            ITERS_NAIVE_SMALL
        };
        let (median, p90) = bench_naive(n, iters);
        naive_medians.push((n, median));
        println!(
            "{:>10} | {:>9} | {:>12} | {:>12}",
            n,
            iters,
            fmt_ns(median),
            fmt_ns(p90)
        );
    }

    println!("\n## Summary (median step cost, growth ratio vs the 1k baseline)\n");
    let engine_base = engine_medians[0].1.as_secs_f64().max(1e-12);
    let naive_base = naive_medians[0].1.as_secs_f64().max(1e-12);
    println!(
        "{:>10} | {:>16} | {:>16}",
        "N_inert", "engine ratio", "naive ratio"
    );
    for i in 0..SCALES.len() {
        println!(
            "{:>10} | {:>16.2} | {:>16.2}",
            engine_medians[i].0,
            engine_medians[i].1.as_secs_f64() / engine_base,
            naive_medians[i].1.as_secs_f64() / naive_base
        );
    }
    println!(
        "\nAn O(Δ)-flat engine's ratio column should stay near 1.0x across scales; the naive \
         oracle's is expected to grow roughly with N_inert. See docs/performance.md for the \
         captured numbers and interpretation."
    );
}
