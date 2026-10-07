//! Wall-clock scaling of `Frontier::apply_delta` (`crates/soc-core/src/calendar.rs`).
//!
//! Measures the per-delta commit scaling across candidate frontier sizes
//! (1k, 10k, 100k candidates). Because `apply_delta` clones the entire
//! candidate BTreeMap on every call (`let mut staged = self.entries.clone()`),
//! the cost of applying a delta of size 1 grows linearly with |frontier|.
//!
//! `harness = false`, `std` only. Run with:
//! `cargo bench -p soc-core --bench world_frontier_delta`.

use std::time::{Duration, Instant};

use brix_canon::{Digest, Domain};
use soc_core::calendar::{Frontier, Key};

const CHECKPOINTS: [usize; 3] = [1_000, 10_000, 50_000];
const SAMPLES: usize = 30;

fn make_digest(tag: &str, n: u64) -> Digest {
    let payload = format!("{tag}_{n}");
    Digest::of(Domain::Value, payload.as_bytes())
}

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

fn main() {
    println!("# Frontier::apply_delta wall-clock scaling (soc-core)\n");
    println!(
        "Method: build a candidate frontier of size N, then measure `apply_delta` for a 1-item delta \
         (1 removal, 1 addition) over {SAMPLES} repetitions.\n"
    );

    println!("{:>10} | {:>14} | {:>14}", "N", "median", "p90");
    println!("{}", "-".repeat(44));

    let mut medians = Vec::new();

    for &n in &CHECKPOINTS {
        let mut frontier = Frontier::new();
        for i in 0..n as u64 {
            let key = Key::new(1, 100, make_digest("cand", i));
            frontier.insert(key, i).unwrap();
        }

        let mut sample_durations = Vec::with_capacity(SAMPLES);
        for s in 0..SAMPLES as u64 {
            let old_key = Key::new(1, 100, make_digest("cand", s));
            let new_key = Key::new(1, 100, make_digest("dyn_cand", s + 1_000_000));

            let removals = [(old_key, s)];
            let additions = [(new_key, s + 999)];

            // `apply_delta` mutates, so each sample needs a fresh frontier. The
            // clone is O(N) and is NOT what we are measuring: do it before the
            // timer starts so only `apply_delta` itself is timed.
            let mut f_sample = frontier.clone();
            let start = Instant::now();
            f_sample.apply_delta(&removals, &additions).unwrap();
            sample_durations.push(start.elapsed());
        }

        let (median, p90) = stats(sample_durations);
        medians.push((n, median));
        println!("{:>10} | {:>14} | {:>14}", n, fmt_ns(median), fmt_ns(p90));
    }

    println!("\n## Summary: Frontier::apply_delta median growth ratio vs smallest checkpoint\n");
    let base = medians[0].1.as_secs_f64().max(1e-12);
    println!(
        "{:>10} | {:>10} | {:>18}",
        "N", "N ratio", "delta time ratio"
    );
    for (n, median) in &medians {
        let n_ratio = *n as f64 / medians[0].0 as f64;
        println!(
            "{:>10} | {:>10.1} | {:>18.2}",
            n,
            n_ratio,
            median.as_secs_f64() / base
        );
    }
}
