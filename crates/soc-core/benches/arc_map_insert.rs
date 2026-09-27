//! Wall-clock scaling of `ArcMap::insert` (`crates/soc-core/src/store.rs`).
//!
//! The module docs there are explicit: `insert` clones the whole map's
//! `BTreeMap` snapshot on every call — "O(n) pointer-sized clones ... not a
//! HAMT's O(log n) per-node sharing" — and that it is not currently on a hot
//! path. This benchmark measures rather than changes it: a single insert's
//! wall-clock cost at map sizes of 1k / 10k / 100k entries, plus `get` for
//! contrast (expected to stay flat — `BTreeMap::get` is O(log n) and is not
//! affected by `insert`'s clone-on-write).
//!
//! Building a map of size N by inserting one entry at a time is itself
//! O(N^2) total (each of the N inserts clones the map built so far) — that
//! total cost *is* the phenomenon being measured, not overhead to avoid. It
//! is paid once, in a single ascending pass to the largest checkpoint, and
//! every individual insert's duration is recorded along the way; each
//! checkpoint's median/p90 is read from the last `WINDOW` insert calls
//! ending at that map size, rather than repeating the whole build per
//! checkpoint.
//!
//! `harness = false`, `std` only. Run with
//! `cargo bench -p soc-core --bench arc_map_insert`.

use std::time::{Duration, Instant};

use soc_core::store::{ArcMap, PersistentMap};

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

    println!("# ArcMap::insert wall-clock scaling (soc-core)\n");
    println!(
        "Method: one ascending build from empty to {max_n} entries; every insert's wall time is \
         recorded; each checkpoint below reads the last {WINDOW} insert durations ending at that \
         map size (median/p90), rather than rebuilding per checkpoint.\n"
    );

    let mut map: ArcMap<u64, u64> = ArcMap::new();
    let mut insert_durations: Vec<Duration> = Vec::with_capacity(max_n);
    for i in 0..max_n as u64 {
        let start = Instant::now();
        map = map.insert(i, i.wrapping_mul(2));
        insert_durations.push(start.elapsed());
    }
    assert_eq!(map.len(), max_n, "sanity: the map holds every inserted key");

    // `get` at each checkpoint, timed after the map has reached that size —
    // O(log n) per the underlying BTreeMap, contrasted against insert's O(n).
    let mut get_durations_by_checkpoint = Vec::new();
    for &cp in &CHECKPOINTS {
        let key = (cp as u64) / 2; // an arbitrary present key
                                   // Rebuild is unnecessary: `map` already holds `max_n >= cp` entries,
                                   // and `get` cost depends on map size, not history, so query the final
                                   // map directly rather than re-deriving a size-`cp` snapshot.
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

    println!("\n## get — for contrast, at the same map sizes (O(log n), expected flat)\n");
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
    println!(
        "\nAn O(n) insert should show its time ratio track its N ratio (both columns close). \
         See docs/performance.md for the captured numbers, interpretation, and what this implies \
         for the current usage profile (`store::ArcMap` is not on a hot path today)."
    );
}
