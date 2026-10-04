//! Wall-clock scaling of `TrieMap::insert` (`crates/soc-core/src/store.rs`).
//!
//! Measures single-insert wall-clock scaling across map sizes of 1k / 10k / 100k entries,
//! plus `get` for comparison.
//!
//! In contrast to `ArcMap::insert` (which clones the entire map, showing O(n) scaling),
//! `TrieMap::insert` performs node-level path copying and incremental Merkle hashing,
//! scaling strictly O(log_16 n) ~ flat/logarithmic.
//!
//! `harness = false`, `std` only. Run with:
//! `cargo bench -p soc-core --bench trie_map_insert`.

use std::time::{Duration, Instant};

use soc_core::store::TrieMap;

const CHECKPOINTS: [usize; 3] = [1_000, 10_000, 100_000];
const WINDOW: usize = 50;

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
    let max_n = *CHECKPOINTS.last().expect("checkpoints non-empty");

    println!("# TrieMap::insert wall-clock scaling (soc-core)\n");
    println!(
        "Method: one ascending build from empty to {max_n} entries; every insert's wall time is \
         recorded; each checkpoint below reads the last {WINDOW} insert durations ending at that \
         map size (median/p90).\n"
    );

    let mut map: TrieMap<u64, u64> = TrieMap::new();
    let mut insert_durations: Vec<Duration> = Vec::with_capacity(max_n);
    for i in 0..max_n as u64 {
        let start = Instant::now();
        map = map.insert(i, i.wrapping_mul(2));
        insert_durations.push(start.elapsed());
    }
    assert_eq!(map.len(), max_n, "sanity: the map holds every inserted key");

    let mut get_durations_by_checkpoint = Vec::new();
    for &cp in &CHECKPOINTS {
        let key = (cp as u64) / 2;
        let mut gets = Vec::with_capacity(2_000);
        for _ in 0..2_000 {
            let start = Instant::now();
            let v = map.get(&key);
            gets.push(start.elapsed());
            assert!(v.is_some());
        }
        get_durations_by_checkpoint.push((cp, stats(gets)));
    }

    println!("## insert — one call's wall time at map size N (window = last {WINDOW} inserts ending at N)\n");
    println!("{:>10} | {:>12} | {:>12}", "N", "median", "p90");
    println!("{}", "-".repeat(40));
    let mut medians = Vec::new();
    for &cp in &CHECKPOINTS {
        let start_idx = cp.saturating_sub(WINDOW);
        let window = insert_durations[start_idx..cp].to_vec();
        let (median, p90) = stats(window);
        medians.push((cp, median));
        println!("{:>10} | {:>12} | {:>12}", cp, fmt_ns(median), fmt_ns(p90));
    }

    println!("\n## get — for contrast, at the same map sizes\n");
    println!("{:>10} | {:>12} | {:>12}", "N", "median", "p90");
    println!("{}", "-".repeat(40));
    for (cp, (median, p90)) in &get_durations_by_checkpoint {
        println!(
            "{:>10} | {:>12} | {:>12}",
            cp,
            fmt_ns(*median),
            fmt_ns(*p90)
        );
    }

    println!("\n## Summary: insert median growth ratio vs the smallest checkpoint\n");
    let base = medians[0].1.as_secs_f64().max(1e-12);
    println!(
        "{:>10} | {:>10} | {:>16}",
        "N", "N ratio", "insert time ratio"
    );
    for (cp, median) in &medians {
        let n_ratio = *cp as f64 / medians[0].0 as f64;
        println!(
            "{:>10} | {:>10.1} | {:>16.2}",
            cp,
            n_ratio,
            median.as_secs_f64() / base
        );
    }
}
